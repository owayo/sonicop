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

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "ruby19".to_owned());
    if style != "ruby19" && style != "ruby19_no_mixed_keys" {
        return;
    }
    let quoted_keys_allowed = context.target_ruby_version() >= QUOTED_KEY_SINCE;

    for node in context.nodes_of("pair") {
        let Some(operator) = hash_rocket(node, context) else {
            continue;
        };
        // A hash is left alone unless every one of its keys can take the new syntax, so that one
        // rocket that has to stay does not leave the hash written in two styles at once.
        if !every_key_takes_the_new_syntax(node, context, quoted_keys_allowed) {
            continue;
        }

        let start = node.start_byte();
        // The opening brace is written **into** the rewrite rather than inserted beside it. An
        // insertion at the byte the rewrite starts at is a second edit at the same position, and
        // `apply_edits` refuses a pair like that -- silently, so the cop reads as having declined
        // to correct at all.
        let wrapping = returned_bare_hash(node, context);
        let opening = if wrapping.is_some() { "{" } else { "" };
        // `argument_without_space?`: `foo:bar => 1` has no space between the selector and the hash,
        // and the old syntax did not need one. `foo` + `bar: 1` runs the two together into
        // `foobar: 1`, which is a different program (and here not a program at all).
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
        // `corrector.wrap(hash_node, '{', '}')`: `return key: value` is not valid Ruby, so a bare
        // hash handed to `return` has to gain braces as it changes syntax. Upstream does this once
        // per hash, on its first pair. Without it the correction turns working code into a syntax
        // error -- and the reparse guard cannot see it, because each pair is a separate offense.
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
                .offense("Use the new Ruby 1.9 hash syntax.", start..operator.end)
                .corrected_by_all(edits),
        );
    }
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
    let pairs: Vec<Node<'_>> = super::nodes::children(list)
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
    let first = super::nodes::children(list)
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

/// `sym_indices?`: whether every key of the hash this pair belongs to is a symbol the new syntax
/// can spell.
fn every_key_takes_the_new_syntax(
    node: Node<'_>,
    context: &RuleContext<'_>,
    quoted_keys_allowed: bool,
) -> bool {
    let Some(container) = node.parent_of(context) else {
        return false;
    };
    let mut cursor = container.walk();
    let pairs: Vec<Node<'_>> = container
        .named_children(&mut cursor)
        .filter(|child| child.kind_str() == "pair")
        .collect();
    !pairs.is_empty()
        && pairs
            .iter()
            .all(|pair| word_symbol_pair(*pair, context, quoted_keys_allowed))
}

/// `word_symbol_pair?`: a symbol key whose name the new syntax accepts.
///
/// A key written with a colon is already a symbol whatever it looks like -- `"a b": 1` reaches
/// RuboCop as a `dsym` -- so only the rocket form has to have its node kind checked.
fn word_symbol_pair(node: Node<'_>, context: &RuleContext<'_>, quoted_keys_allowed: bool) -> bool {
    let Some(key) = node.field("key") else {
        return false;
    };
    let is_symbol = hash_rocket(node, context).is_none()
        || matches!(key.kind_str(), "simple_symbol" | "delimited_symbol");
    is_symbol && acceptable_19_syntax_symbol(context.source.node_text(key), quoted_keys_allowed)
}

fn acceptable_19_syntax_symbol(text: &str, quoted_keys_allowed: bool) -> bool {
    let name = text.strip_prefix(':').unwrap_or(text);
    if PLAIN_SYMBOL.is_match(name) {
        return true;
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
