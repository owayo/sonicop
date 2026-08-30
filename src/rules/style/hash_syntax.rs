use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

/// The symbol names the new syntax can spell without quoting them, as
/// `acceptable_19_syntax_symbol?` matches them. A trailing `?` or `!` is fine; a trailing `=` is
/// not, because `{ foo=: 1 }` is not valid Ruby.
static PLAIN_SYMBOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)^[_a-z][A-Za-z0-9_]*[?!]?$").unwrap());

/// The quoted form only became a legal hash key in Ruby 2.2.
const QUOTED_KEY_SINCE: RubyVersion = RubyVersion::new(2, 2);

/// `EnforcedShorthandSyntax`: what to do about Ruby 3.1's `{ foo: }`, which is a separate axis
/// from `EnforcedStyle` and reaches the same pairs.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Shorthand {
    Either,
    Always,
    Never,
    Consistent,
    EitherConsistent,
}

/// Value omission is Ruby 3.1 syntax. Below that every pair is left alone, whatever the setting
/// says, because the shorter form would not parse.
const OMISSION_SINCE: RubyVersion = RubyVersion::new(3, 1);

const MSG_19: &str = "Use the new Ruby 1.9 hash syntax.";
const MSG_HASH_ROCKETS: &str = "Use hash rockets syntax.";
const MSG_NO_MIXED_KEYS: &str = "Don't mix styles in the same hash.";
const OMIT_MSG: &str = "Omit the hash value.";
const EXPLICIT_MSG: &str = "Include the hash value.";
const MIX_PREFIX: &str = "Do not mix explicit and implicit hash values.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    check_shorthand(context, offenses);
    check_syntax(context, offenses);
}

fn check_syntax(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "ruby19".to_owned());
    let quoted_keys_allowed = context.target_ruby_version() >= QUOTED_KEY_SINCE;
    let prefer_rockets_for_non_alnum = context
        .setting::<bool>("PreferHashRocketsForNonAlnumEndingSymbols")
        .unwrap_or(false);

    for pairs in hash_groups(context) {
        // `sym_indices?`: a hash is left in the old syntax unless every one of its keys can take
        // the new one, so that one rocket that has to stay does not leave the hash written in two
        // styles at once.
        let sym_indices = pairs.iter().all(|pair| {
            word_symbol_pair(
                *pair,
                context,
                quoted_keys_allowed,
                prefer_rockets_for_non_alnum,
            )
        });
        // `force_hash_rockets?`: a hash holding a symbol value is written in the old syntax
        // whatever the style says, so that `{ a: :b }` does not read as two different things.
        let force_hash_rockets = context
            .setting::<bool>("UseHashRocketsWithSymbolValues")
            .unwrap_or(false)
            && pairs.iter().any(|pair| {
                pair.field("value").is_some_and(|value| {
                    matches!(value.kind_str(), "simple_symbol" | "delimited_symbol")
                })
            });
        // Which delimiter the style rejects, and what it says about it. `check(pairs, delim, msg)`
        // upstream.
        let (reject_colon, message) = match style.as_str() {
            _ if force_hash_rockets => (true, MSG_HASH_ROCKETS),
            "hash_rockets" => (true, MSG_HASH_ROCKETS),
            "ruby19_no_mixed_keys" if sym_indices => (false, MSG_19),
            "ruby19_no_mixed_keys" => (true, MSG_NO_MIXED_KEYS),
            "no_mixed_keys" if sym_indices => {
                // `pairs.first.inverse_delimiter`: whichever way the hash opens, the others have
                // to follow, so what is rejected is the opposite of the first pair's delimiter.
                let first_colon = pair_delimiter(context, pairs[0]).is_some_and(|(_, colon)| colon);
                (!first_colon, MSG_NO_MIXED_KEYS)
            }
            "no_mixed_keys" => (true, MSG_NO_MIXED_KEYS),
            _ if !sym_indices => continue,
            _ => (false, MSG_19),
        };

        for &node in &pairs {
            let Some((operator, is_colon)) = pair_delimiter(context, node) else {
                continue;
            };
            if is_colon != reject_colon {
                continue;
            }
            if is_colon {
                offenses.push(
                    context
                        .offense(message, node.start_byte()..operator.end)
                        .corrected_by_all(rocket_edits(context, node, &operator)),
                );
                continue;
            }

            let start = node.start_byte();
            // The opening brace is written **into** the rewrite rather than inserted beside it. An
            // insertion at the byte the rewrite starts at is a second edit at the same position,
            // and `apply_edits` refuses a pair like that -- silently, so the cop reads as having
            // declined to correct at all.
            let wrapping = returned_bare_hash(node, context);
            let opening = if wrapping.is_some() { "{" } else { "" };
            // `argument_without_space?`: `foo:bar => 1` has no space between the selector and the
            // hash, and the old syntax did not need one. `foo` + `bar: 1` runs the two together
            // into `foobar: 1`, which is a different program (and here not a program at all).
            let spacing = if argument_without_space(node, context) {
                " "
            } else {
                ""
            };
            let mut edits = vec![Edit {
                start,
                end: whitespace_end(context, operator.end),
                replacement: format!("{spacing}{opening}{}: ", key_name(node, context)),
                safe: true,
            }];
            // `corrector.wrap(hash_node, '{', '}')`: `return key: value` is not valid Ruby, so a
            // bare hash handed to `return` has to gain braces as it changes syntax. Upstream does
            // this once per hash, on its first pair. Without it the correction turns working code
            // into a syntax error -- and the reparse guard cannot see it, because each pair is a
            // separate offense.
            if let Some((_, close)) = wrapping {
                edits.push(Edit {
                    start: close,
                    end: close,
                    replacement: "}".to_owned(),
                    safe: true,
                });
            }
            offenses.push(
                context
                    .offense(message, start..operator.end)
                    .corrected_by_all(edits),
            );
        }
    }
}

/// `autocorrect_hash_rockets`: the key gains the `:` the symbol literal needs and the rocket that
/// replaces the colon, and the colon itself goes with the space around it.
fn rocket_edits(
    context: &RuleContext<'_>,
    pair: Node<'_>,
    operator: &std::ops::Range<usize>,
) -> Vec<Edit> {
    let Some(key) = pair.field("key") else {
        return Vec::new();
    };
    let key_source = context.source.node_text(key);
    let mut replacement = format!(":{key_source} => ");
    // `key_with_hash_rocket += pair_node.key.source if pair_node.value_omission?`: the old syntax
    // has no short form, so the value the colon left out has to be written back.
    if pair.field("value").is_none() {
        replacement.push_str(key_source);
    }
    vec![
        Edit {
            start: key.start_byte(),
            end: key.end_byte(),
            replacement,
            safe: true,
        },
        Edit {
            start: operator.start,
            end: whitespace_end(context, operator.end),
            replacement: String::new(),
            safe: true,
        },
    ]
}

/// The pair's delimiter: its span, and whether it is the colon of the new syntax.
fn pair_delimiter(
    context: &RuleContext<'_>,
    pair: Node<'_>,
) -> Option<(std::ops::Range<usize>, bool)> {
    let key = pair.field("key")?;
    let after = key.end_byte();
    let text = context.source.text();
    let end = pair
        .field("value")
        .map_or(pair.end_byte(), |v| v.start_byte());
    let between = text.get(after..end)?;
    if let Some(offset) = between.find("=>") {
        return Some((after + offset..after + offset + "=>".len(), false));
    }
    let offset = between.find(':')?;
    Some((after + offset..after + offset + 1, true))
}

/// `HashShorthandSyntax`: the value-omission axis.
///
/// `always` and `never` read one pair at a time -- omit every value that can go, or write every
/// one back. `consistent` and `either_consistent` read the hash instead, because what they forbid
/// is the mixture: a hash whose pairs are not all written the same way is reported on the pairs
/// that break the majority, and which side that is depends on whether some value in the hash
/// *cannot* be omitted.
fn check_shorthand(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let shorthand = match context
        .setting::<String>("EnforcedShorthandSyntax")
        .as_deref()
    {
        Some("always") => Shorthand::Always,
        Some("never") => Shorthand::Never,
        Some("consistent") => Shorthand::Consistent,
        Some("either_consistent") => Shorthand::EitherConsistent,
        _ => Shorthand::Either,
    };
    if shorthand == Shorthand::Either || context.target_ruby_version() < OMISSION_SINCE {
        return;
    }
    match shorthand {
        Shorthand::Always | Shorthand::Never => {
            for pair in context.nodes_of("pair") {
                if !in_hash_literal(pair, context) {
                    continue;
                }
                on_pair(context, pair, shorthand, offenses);
            }
        }
        _ => {
            for pairs in hash_groups(context) {
                mixed_shorthand(context, &pairs, shorthand, offenses);
            }
        }
    }
}

/// `on_pair` under `always` and `never`.
fn on_pair(
    context: &RuleContext<'_>,
    pair: Node<'_>,
    shorthand: Shorthand,
    offenses: &mut Vec<Offense>,
) {
    let Some(key) = pair.field("key") else {
        return;
    };
    let key_source = context.source.node_text(key);
    if shorthand == Shorthand::Always {
        if omits_value(pair) || require_hash_value(context, pair) {
            return;
        }
        register_shorthand(context, pair, OMIT_MSG, format!("{key_source}:"), offenses);
    } else {
        if !omits_value(pair) {
            return;
        }
        register_shorthand(
            context,
            pair,
            EXPLICIT_MSG,
            format!("{key_source}: {key_source}"),
            offenses,
        );
    }
}

/// `on_hash_for_mixed_shorthand` and the two checks it dispatches to.
fn mixed_shorthand(
    context: &RuleContext<'_>,
    pairs: &[Node<'_>],
    shorthand: Shorthand,
    offenses: &mut Vec<Offense>,
) {
    // `breakdown_value_types_of_hash`.
    let (mut omitted, mut needed, mut omittable) = (Vec::new(), Vec::new(), Vec::new());
    for &pair in pairs {
        if omits_value(pair) {
            omitted.push(pair);
        } else if require_hash_value(context, pair) {
            needed.push(pair);
        } else {
            omittable.push(pair);
        }
    }
    let kinds = usize::from(!omitted.is_empty())
        + usize::from(!needed.is_empty())
        + usize::from(!omittable.is_empty());

    if kinds > 1 {
        // `mixed_shorthand_syntax_check`: a hash holding a value that cannot go is written out in
        // full, and one where every value could go is written short.
        let (targets, message) = if needed.is_empty() {
            (&omittable, format!("{MIX_PREFIX} {OMIT_MSG}"))
        } else {
            (&omitted, format!("{MIX_PREFIX} {EXPLICIT_MSG}"))
        };
        let omit = needed.is_empty();
        for &pair in targets {
            let Some(key) = pair.field("key") else {
                continue;
            };
            let key_source = context.source.node_text(key);
            let replacement = if omit {
                format!("{key_source}:")
            } else {
                format!("{key_source}: {key_source}")
            };
            register_shorthand(context, pair, &message, replacement, offenses);
        }
        return;
    }

    // `no_mixed_shorthand_syntax_check`.
    if !needed.is_empty() {
        return;
    }
    // `ignore_explicit_omissible_hash_shorthand_syntax?`: under `either_consistent` a hash whose
    // values could all be omitted is consistent as it stands, so writing them out is accepted.
    if shorthand == Shorthand::EitherConsistent && omitted.is_empty() {
        return;
    }
    for &pair in &omittable {
        let Some(key) = pair.field("key") else {
            continue;
        };
        let key_source = context.source.node_text(key);
        register_shorthand(context, pair, OMIT_MSG, format!("{key_source}:"), offenses);
    }
}

/// `register_offense`: the offense sits on the value -- and on a pair that has none, on the key
/// the parser puts the implicit value's range over.
fn register_shorthand(
    context: &RuleContext<'_>,
    pair: Node<'_>,
    message: impl Into<String>,
    replacement: String,
    offenses: &mut Vec<Offense>,
) {
    let range = match pair.field("value") {
        Some(value) => value.byte_range(),
        None => match pair.field("key") {
            Some(key) => key.byte_range(),
            None => pair.byte_range(),
        },
    };
    // The closing bracket goes **into** the rewrite: an insertion at the byte the rewrite ends on
    // is a second edit at the same position, which `apply_edits` refuses -- silently, so the cop
    // reads as having declined to correct at all.
    let opening = parentheses_the_omission_needs(context, pair);
    let replacement = match opening.is_some() {
        true => format!("{replacement})"),
        false => replacement,
    };
    let mut edits = vec![Edit {
        start: pair.start_byte(),
        end: pair.end_byte(),
        replacement,
        safe: true,
    }];
    edits.extend(opening);
    offenses.push(
        context
            .offense(message.into(), range)
            .corrected_by_all(edits),
    );
}

/// `def_node_that_require_parentheses`: dropping the value from a parenthesis-less call would
/// leave `foo bar:`, which is not the same program. Upstream adds the parentheses in the same
/// pass, replacing the space after the selector and closing after the last argument.
fn parentheses_the_omission_needs(context: &RuleContext<'_>, pair: Node<'_>) -> Option<Edit> {
    let hash = pair.parent_of(context)?;
    if hash.kind_str() != "hash" && hash.kind_str() != "argument_list" {
        return None;
    }
    // Only the last pair carries the closing bracket, and only when it is the one being omitted.
    let pairs = super::nodes::children_in(hash, context);
    if pairs.last().is_none_or(|last| last.id() != pair.id()) {
        return None;
    }
    // A braced hash needs nothing: `{ bar: }` already reads as one.
    if hash.kind_str() == "hash"
        && hash
            .child(0)
            .is_some_and(|first| context.source.node_text(first) == "{")
    {
        return None;
    }
    let call = find_ancestor_method_dispatch(context, hash)?;
    let (selector, arguments) = (call.field("method")?, call.field("arguments")?);
    // `dispatch_node.parenthesized?`
    if arguments
        .child(0)
        .is_some_and(|first| context.source.node_text(first) == "(")
    {
        return None;
    }
    let first_argument = *super::nodes::children_in(arguments, context).first()?;
    // The closing bracket only lands right when the omitted pair ends the argument list.
    if arguments.end_byte() != pair.end_byte() {
        return None;
    }
    // `return if last_expression?(dispatch) && !requires_parentheses_context?(dispatch)`: a call
    // that ends its body needs no help, because `foo bar:` there still parses. One with anything
    // written after it does -- `foo value:` would swallow the next line.
    if last_expression(call, context) && !requires_parentheses_context(call, context) {
        return None;
    }
    Some(Edit {
        start: selector.end_byte(),
        end: first_argument.start_byte(),
        replacement: "(".to_owned(),
        safe: true,
    })
}

/// `find_ancestor_method_dispatch_node`: the call, `super` or `yield` the hash is an argument of.
fn find_ancestor_method_dispatch<'tree>(
    context: &'tree RuleContext<'_>,
    hash: Node<'tree>,
) -> Option<Node<'tree>> {
    let mut node = hash;
    while let Some(parent) = node.parent_of(context) {
        match parent.kind_str() {
            "call" | "super" | "yield" => return Some(parent),
            "argument_list" => node = parent,
            _ => return None,
        }
    }
    None
}

/// `requires_parentheses_context?`: the call sits where a bare `foo bar:` would be read as part of
/// the surrounding expression.
/// `last_expression?`: nothing is written after the call, once an enclosing assignment is followed
/// out to whatever holds *it*.
fn last_expression(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.next_named_sibling().is_some() {
        return false;
    }
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "assignment" | "operator_assignment") {
            // `return last_expression?(assignment.parent) if assignment.parent&.assignment?`.
            return match ancestor
                .parent_of(context)
                .filter(|parent| matches!(parent.kind_str(), "assignment" | "operator_assignment"))
            {
                Some(parent) => last_expression(parent, context),
                None => ancestor.next_named_sibling().is_none(),
            };
        }
        current = ancestor.parent_of(context);
    }
    true
}

fn requires_parentheses_context(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `node.parent`: the grammar interposes an `argument_list` upstream has no node for, so the
    // parent of a nested call is that list rather than the call it is an argument of.
    let mut parent = call.parent_of(context);
    while parent.is_some_and(|node| node.kind_str() == "argument_list") {
        parent = parent.and_then(|node| node.parent_of(context));
    }
    parent.is_some_and(|parent| {
        matches!(
            parent.kind_str(),
            "call" | "if" | "unless" | "super" | "until" | "while" | "yield"
        )
    })
}

/// `node.value_omission?`.
fn omits_value(pair: Node<'_>) -> bool {
    pair.field("value").is_none()
}

/// `!pair_node.parent.hash_type?` inverted. A braceless hash has its pairs sitting in the argument
/// list, where upstream's parser still builds a `hash` around them; a hash *pattern* does not.
fn in_hash_literal(pair: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `Hash[foo: foo]` parks its pairs straight under the `element_reference`; upstream still
    // builds a `hash` around them there, as it does for a braceless argument list.
    pair.parent_of(context).is_some_and(|parent| {
        matches!(
            parent.kind_str(),
            "hash" | "argument_list" | "element_reference"
        )
    })
}

/// The pairs of each hash in the file, in source order.
fn hash_groups<'tree>(context: &'tree RuleContext<'_>) -> Vec<Vec<Node<'tree>>> {
    let mut groups: Vec<(usize, Vec<Node<'tree>>)> = Vec::new();
    for pair in context.nodes_of("pair") {
        let Some(parent) = pair.parent_of(context) else {
            continue;
        };
        if !matches!(
            parent.kind_str(),
            "hash" | "argument_list" | "element_reference"
        ) {
            continue;
        }
        match groups.iter_mut().find(|(id, _)| *id == parent.id()) {
            Some((_, list)) => list.push(pair),
            None => groups.push((parent.id(), vec![pair])),
        }
    }
    groups.into_iter().map(|(_, list)| list).collect()
}

/// `require_hash_value?`: whether the value has to stay written out.
fn require_hash_value(context: &RuleContext<'_>, pair: Node<'_>) -> bool {
    let Some(key) = pair.field("key") else {
        return true;
    };
    if !is_symbol_key(key) || require_hash_value_around_hash_literal(context, pair) {
        return true;
    }
    let Some(value) = pair.field("value") else {
        return true;
    };
    // `hash_value.type?(:send, :lvar)`: only a bare name can be the value the key stands in for.
    if !matches!(value.kind_str(), "identifier" | "call") {
        return true;
    }
    let key_source = context.source.node_text(key);
    // A key ending in `!` or `?` cannot be shortened: `{ foo?: }` is not a method call on the
    // omitted side.
    key_source != context.source.node_text(value)
        || key_source.ends_with('!')
        || key_source.ends_with('?')
}

/// `node.key.sym_type?`. A key written before a colon is a symbol whatever it looks like.
fn is_symbol_key(key: Node<'_>) -> bool {
    matches!(
        key.kind_str(),
        "hash_key_symbol" | "simple_symbol" | "delimited_symbol"
    )
}

/// `require_hash_value_for_around_hash_literal?`: a braceless hash handed to a call written
/// without parentheses, inside a modifier form, keeps its values -- omitting one there changes
/// where the parser thinks the argument list ends.
fn require_hash_value_around_hash_literal(context: &RuleContext<'_>, pair: Node<'_>) -> bool {
    let Some(parent) = pair.parent_of(context) else {
        return false;
    };
    // `!node.parent.braces?`: a hash written with braces is delimited already.
    if parent.kind_str() != "argument_list" {
        return false;
    }
    let Some(dispatch) = ancestor_method_dispatch(context, parent) else {
        return false;
    };
    // `use_element_of_hash_literal_as_receiver?`: `{ value: }.do_something` is fine.
    if dispatch.field("receiver") == Some(parent) {
        return false;
    }
    if is_parenthesized_call(context, dispatch) {
        return false;
    }
    // `use_modifier_form_without_parenthesized_method_call?`.
    let mut current = dispatch;
    while let Some(ancestor) = current.parent_of(context) {
        if is_modifier_form(ancestor, context) {
            return true;
        }
        current = ancestor;
    }
    false
}

/// `find_ancestor_method_dispatch_node`: the call the hash is an argument of, unless that "call"
/// is an index read.
fn ancestor_method_dispatch<'tree>(
    context: &'tree RuleContext<'_>,
    list: Node<'tree>,
) -> Option<Node<'tree>> {
    let ancestor = list.parent_of(context)?;
    if !matches!(
        ancestor.kind_str(),
        "call" | "super" | "yield" | "method_call"
    ) {
        return None;
    }
    // `brackets?`: `foo[bar: 1]` is not a dispatch the omission can confuse.
    let method = ancestor
        .field("method")
        .map(|node| context.source.node_text(node));
    if matches!(method, Some("[]" | "[]=")) {
        return None;
    }
    Some(ancestor)
}

fn is_parenthesized_call(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.field("arguments")
        .is_some_and(|list| context.source.node_text(list).starts_with('('))
}

/// `modifier_form?`: a trailing `if`, `unless`, `while`, `until` or `rescue`.
fn is_modifier_form(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !matches!(
        node.kind_str(),
        "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier" | "rescue_modifier"
    ) {
        return false;
    }
    let _ = context;
    true
}

/// `hash_node.parent&.return_type? && !hash_node.braces?`: the span of a braceless hash written
/// straight after `return`, given only for its first pair so the braces go on once.
///
/// The grammar writes no `hash` node for it -- the pairs sit directly in the argument list -- so
/// the hash is "from the first pair to the last".
fn returned_bare_hash(node: Node<'_>, context: &RuleContext<'_>) -> Option<(usize, usize)> {
    let list = node.parent_of(context)?;
    if list.kind_str() != "argument_list" {
        return None;
    }
    if context.parent(list)?.kind_str() != "return" {
        return None;
    }
    let pairs: Vec<Node<'_>> = super::nodes::children_in(list, context)
        .into_iter()
        .filter(|child| child.kind_str() == "pair")
        .collect();
    let (first, last) = (pairs.first()?, pairs.last()?);
    (first.id() == node.id()).then(|| (first.start_byte(), last.end_byte()))
}

/// `argument_without_space?`: the hash this pair belongs to starts exactly where the call's
/// selector ends, so the new syntax needs a space that the old one did not.
fn argument_without_space(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(list) = node.parent_of(context) else {
        return false;
    };
    if list.kind_str() != "argument_list" {
        return false;
    }
    // The comparison is against **the hash**, not the argument list. A parenthesized call has its
    // list start at the `(`, which always sits right after the selector, so comparing the list
    // would put a space into every `func(:a => 0)`.
    let first = super::nodes::children_in(list, context)
        .into_iter()
        .find(|child| child.kind_str() == "pair");
    let Some(first) = first else {
        return false;
    };
    context
        .parent(list)
        .and_then(|call| call.field("method"))
        .is_some_and(|method| method.end_byte() == first.start_byte())
}

/// The span of the pair's `=>`, or `None` when the pair is already written with a colon.
fn hash_rocket(node: Node<'_>, context: &RuleContext<'_>) -> Option<std::ops::Range<usize>> {
    let key = node.field("key")?;
    let value = node.field("value")?;
    let between = context.source.slice(key.end_byte()..value.start_byte());
    let offset = between.find("=>")?;
    Some(key.end_byte() + offset..key.end_byte() + offset + "=>".len())
}

/// The key without the leading `:`, which is what the new syntax puts in front of the colon.
fn key_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    let text = node
        .field("key")
        .map_or("", |key| context.source.node_text(key));
    text.strip_prefix(':').unwrap_or(text)
}

/// `word_symbol_pair?`: a symbol key whose name the new syntax accepts.
///
/// A key written with a colon is already a symbol whatever it looks like -- `"a b": 1` reaches
/// RuboCop as a `dsym` -- so only the rocket form has to have its node kind checked.
fn word_symbol_pair(
    node: Node<'_>,
    context: &RuleContext<'_>,
    quoted_keys_allowed: bool,
    prefer_rockets_for_non_alnum: bool,
) -> bool {
    let Some(key) = node.field("key") else {
        return false;
    };
    let is_symbol = hash_rocket(node, context).is_none()
        || matches!(key.kind_str(), "simple_symbol" | "delimited_symbol");
    is_symbol
        && acceptable_19_syntax_symbol(
            context.source.node_text(key),
            quoted_keys_allowed,
            prefer_rockets_for_non_alnum,
        )
}

fn acceptable_19_syntax_symbol(
    text: &str,
    quoted_keys_allowed: bool,
    prefer_rockets_for_non_alnum: bool,
) -> bool {
    let name = text.strip_prefix(':').unwrap_or(text);
    if PLAIN_SYMBOL.is_match(name) {
        return !(prefer_rockets_for_non_alnum
            && matches!(name.as_bytes().last(), Some(b'?' | b'!')));
    }
    quoted_keys_allowed
        && name.len() >= 2
        && ((name.starts_with('\'') && name.ends_with('\''))
            || (name.starts_with('"') && name.ends_with('"')))
}

/// The end of the run of blanks after `offset`, which the correction swallows so that the colon it
/// leaves behind keeps a single space in front of the value.
///
/// RuboCop's `range_with_surrounding_space` takes the spaces and tabs first and only then the line
/// breaks, so the indentation of a following line is left where it is.
fn whitespace_end(context: &RuleContext<'_>, offset: usize) -> usize {
    support::final_pos(context.source.text(), offset, true, false, true, false)
}
