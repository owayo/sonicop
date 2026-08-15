use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

const FORWARDING_MSG: &str = "Use shorthand syntax `...` for arguments forwarding.";
const ARGS_MSG: &str = "Use anonymous positional arguments forwarding (`*`).";
const KWARGS_MSG: &str = "Use anonymous keyword arguments forwarding (`**`).";
const BLOCK_MSG: &str = "Use anonymous block arguments forwarding (`&`).";

/// `minimum_target_ruby_version 2.7`.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);

/// The version `...` stops needing every one of the three arguments to be forwarded.
const ANONYMOUS: RubyVersion = RubyVersion::new(3, 2);

/// The version an anonymous argument may be forwarded from inside a block.
const IN_BLOCK: RubyVersion = RubyVersion::new(3, 4);

/// The version anonymous block forwarding arrived in.
const BLOCK_FORWARDING: RubyVersion = RubyVersion::new(3, 1);

/// The version `...` stopped needing the arguments it forwards to be the only ones.
const POST_SPLAT: RubyVersion = RubyVersion::new(3, 0);

/// The kinds that hold a block's own body, whose ancestors decide whether an anonymous argument may
/// be forwarded at all.
const BLOCKS: &[&str] = &["block", "do_block"];

/// How much of the call's argument list is being forwarded.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Classification {
    /// Everything, which `...` says in one token.
    All,
    /// Everything, already written as anonymous arguments.
    AllAnonymous,
    /// Some of it, which only the anonymous forms can say.
    RestOrKwrest,
}

/// The three parameters a definition can forward, kept only where their names are ones the
/// configuration calls redundant.
struct Forwardable<'tree> {
    rest: Option<Node<'tree>>,
    kwrest: Option<Node<'tree>>,
    block: Option<Node<'tree>>,
}

/// What one call inside the definition forwards.
struct Classified<'tree> {
    send: Node<'tree>,
    kind: Classification,
    rest: Option<Node<'tree>>,
    kwrest: Option<Node<'tree>>,
    block: Option<Node<'tree>>,
}

/// The settings the cop reads, gathered once per file.
struct Settings {
    ruby: RubyVersion,
    allow_only_rest: bool,
    use_anonymous: bool,
    explicit_block_name: bool,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let settings = Settings {
        ruby: context.target_ruby_version(),
        allow_only_rest: context.setting("AllowOnlyRestArgument").unwrap_or(true),
        use_anonymous: context.setting("UseAnonymousForwarding").unwrap_or(false),
        // `explicit_block_name?`: the naming cop asks for a name, so taking it away would fight it.
        explicit_block_name: context.cop_enabled("Naming/BlockForwarding")
            && context
                .setting_of::<String>("Naming/BlockForwarding", "EnforcedStyle")
                .as_deref()
                == Some("explicit"),
    };
    if settings.ruby < MINIMUM {
        return;
    }
    let redundant = (
        names(context, "RedundantRestArgumentNames", "*"),
        names(context, "RedundantKeywordRestArgumentNames", "**"),
        names(context, "RedundantBlockArgumentNames", "&"),
    );
    for definition in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(body) = definition.field("body") else {
            continue;
        };
        let parameters = definition.field("parameters");
        let forwardable = forwardable(parameters, &redundant, context);
        let referenced = referenced_names(body, &forwardable, context);
        let classified: Vec<Classified<'_>> = send_nodes(body)
            .into_iter()
            .filter_map(|send| {
                classify(
                    definition,
                    send,
                    &forwardable,
                    &referenced,
                    &settings,
                    context,
                )
            })
            .collect();
        if classified.is_empty() {
            continue;
        }
        if classified.iter().all(|entry| {
            matches!(
                entry.kind,
                Classification::All | Classification::AllAnonymous
            )
        }) {
            forward_all_offenses(
                definition,
                parameters,
                &classified,
                &forwardable,
                &settings,
                context,
                offenses,
            );
        } else if settings.ruby >= ANONYMOUS {
            anonymous_offenses(
                parameters,
                &classified,
                &forwardable,
                &settings,
                context,
                offenses,
            );
        }
    }
}

/// The arguments a call, a `yield` or an index read was written with. Only a call keeps them in a
/// field of its own: a `yield` holds the list as a plain child, and an index read holds the indices
/// beside the object.
fn argument_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    if node.kind_str() == "element_reference" {
        let object = node.field("object").map(|object| object.id());
        return named_children(node)
            .into_iter()
            .filter(|child| Some(child.id()) != object)
            .collect();
    }
    let list = node.field("arguments").or_else(|| {
        named_children(node)
            .into_iter()
            .find(|child| child.kind_str() == "argument_list")
    });
    list.map(named_children).unwrap_or_default()
}

/// The arguments grouped the way upstream's `send` holds them: a trailing run of `key: value` pairs
/// and double splats is the one `hash` its patterns look inside.
fn grouped<'tree>(node: Node<'tree>) -> Vec<Vec<Node<'tree>>> {
    let mut groups: Vec<Vec<Node<'tree>>> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for child in argument_nodes(node) {
        if child.kind_str() == "comment" {
            continue;
        }
        if matches!(child.kind_str(), "pair" | "hash_splat_argument") {
            hash.push(child);
            continue;
        }
        if !hash.is_empty() {
            groups.push(std::mem::take(&mut hash));
        }
        groups.push(vec![child]);
    }
    if !hash.is_empty() {
        groups.push(hash);
    }
    groups
}

/// `redundant_named_arg`: the spellings of a parameter the configuration is willing to take the name
/// off, which is the bare keyword and the keyword followed by one of the configured names.
fn names(context: &RuleContext<'_>, key: &str, keyword: &str) -> Vec<String> {
    let mut spellings: Vec<String> = context
        .setting::<Vec<String>>(key)
        .unwrap_or_default()
        .into_iter()
        .map(|name| format!("{keyword}{name}"))
        .collect();
    spellings.push(keyword.to_owned());
    spellings
}

/// `extract_forwardable_args` together with `redundant_forwardable_named_args`.
fn forwardable<'tree>(
    parameters: Option<Node<'tree>>,
    redundant: &(Vec<String>, Vec<String>, Vec<String>),
    context: &RuleContext<'_>,
) -> Forwardable<'tree> {
    let list = parameters.map(named_children).unwrap_or_default();
    let find = |kind: &str, spellings: &[String]| {
        list.iter().copied().find(|parameter| {
            parameter.kind_str() == kind
                && spellings
                    .iter()
                    .any(|spelling| spelling == context.source.node_text(*parameter))
        })
    };
    Forwardable {
        rest: find("splat_parameter", &redundant.0),
        kwrest: find("hash_splat_parameter", &redundant.1),
        block: find("block_parameter", &redundant.2),
    }
}

/// The name a parameter binds, which is what a forwarded argument has to spell.
fn parameter_name<'a>(
    parameter: Option<Node<'_>>,
    context: &'a RuleContext<'_>,
) -> Option<&'a str> {
    let name = parameter?.field("name")?;
    Some(context.source.node_text(name))
}

/// `non_splat_or_block_pass_lvar_references`: the names the body reads for something other than
/// forwarding them, which is what keeps a name from being taken away.
fn referenced_names(
    body: Node<'_>,
    forwardable: &Forwardable<'_>,
    context: &RuleContext<'_>,
) -> Vec<String> {
    let watched: Vec<&str> = [
        parameter_name(forwardable.rest, context),
        parameter_name(forwardable.kwrest, context),
        parameter_name(forwardable.block, context),
    ]
    .into_iter()
    .flatten()
    .collect();
    if watched.is_empty() {
        return Vec::new();
    }
    let mut referenced = Vec::new();
    let mut stack = named_children(body);
    while let Some(node) = stack.pop() {
        stack.extend(named_children(node));
        if node.kind_str() != "identifier" {
            continue;
        }
        let text = context.source.node_text(node);
        if !watched.contains(&text) || referenced.iter().any(|name| name == text) {
            continue;
        }
        // A name written as the argument of a splat, a double splat or a block pass is being
        // forwarded rather than read, and a method of that name is no local variable at all.
        if is_forwarding_use(node, context) || is_method_name(node, context) {
            continue;
        }
        referenced.push(text.to_owned());
    }
    referenced
}

fn is_forwarding_use(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        matches!(
            parent.kind_str(),
            "splat_argument" | "hash_splat_argument" | "block_argument"
        )
    })
}

fn is_method_name(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        parent
            .field("method")
            .is_some_and(|method| method.id() == node.id())
    })
}

/// `node.each_descendant(:call, :super, :yield)`: a bare `super` is a `zsuper` upstream and forwards
/// on its own, so it is no part of this.
fn send_nodes<'tree>(body: Node<'tree>) -> Vec<Node<'tree>> {
    let mut sends = Vec::new();
    let mut stack = named_children(body);
    while let Some(node) = stack.pop() {
        stack.extend(named_children(node));
        // An index read is a `send` upstream, so `self[*args]` forwards just as a call does.
        if matches!(node.kind_str(), "call" | "yield" | "element_reference") {
            sends.push(node);
        }
    }
    sends.sort_by_key(Node::start_byte);
    sends
}

/// `SendNodeClassifier#classification` together with what it hands back alongside it.
fn classify<'tree>(
    definition: Node<'tree>,
    send: Node<'tree>,
    forwardable: &Forwardable<'tree>,
    referenced: &[String],
    settings: &Settings,
    context: &'tree RuleContext<'_>,
) -> Option<Classified<'tree>> {
    let list = grouped(send);
    let rest = (!is_referenced(forwardable.rest, referenced, context))
        .then(|| forwarded_rest(&list, forwardable.rest, context))
        .flatten();
    let kwrest = (!is_referenced(forwardable.kwrest, referenced, context))
        .then(|| forwarded_kwrest(&list, forwardable.kwrest, context))
        .flatten();
    let block = (!is_referenced(forwardable.block, referenced, context))
        .then(|| forwarded_block(&list, forwardable.block, context))
        .flatten();
    if rest.is_none() && kwrest.is_none() && block.is_none() {
        return None;
    }
    let kind = if is_all_anonymous(definition, send, context) {
        Classification::AllAnonymous
    } else if can_forward_all(
        definition,
        &list,
        forwardable,
        referenced,
        (rest, kwrest, block),
        settings,
        context,
    ) {
        Classification::All
    } else {
        Classification::RestOrKwrest
    };
    Some(Classified {
        send,
        kind,
        rest,
        kwrest,
        block,
    })
}

fn is_referenced(
    parameter: Option<Node<'_>>,
    referenced: &[String],
    context: &RuleContext<'_>,
) -> bool {
    parameter_name(parameter, context)
        .is_some_and(|name| referenced.iter().any(|seen| seen == name))
}

/// `(splat (lvar %1))`.
fn forwarded_rest<'tree>(
    list: &[Vec<Node<'tree>>],
    parameter: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let name = parameter_name(parameter, context)?;
    list.iter()
        .filter_map(|group| group.first())
        .copied()
        .find(|argument| {
            argument.kind_str() == "splat_argument" && child_name(*argument, context) == Some(name)
        })
}

/// `(hash <$(kwsplat (lvar %1)) ...>)`: the double splat written inside the hash the trailing keyword
/// arguments build.
fn forwarded_kwrest<'tree>(
    list: &[Vec<Node<'tree>>],
    parameter: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let name = parameter_name(parameter, context)?;
    list.iter().flatten().copied().find(|part| {
        part.kind_str() == "hash_splat_argument" && child_name(*part, context) == Some(name)
    })
}

/// `(block_pass {(lvar %1) nil?})`.
fn forwarded_block<'tree>(
    list: &[Vec<Node<'tree>>],
    parameter: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let name = parameter_name(parameter, context);
    list.iter()
        .filter_map(|group| group.first())
        .copied()
        .find(|argument| {
            if argument.kind_str() != "block_argument" {
                return false;
            }
            // `{(lvar %1) nil?}`: the name the definition binds, or nothing at all. `&:symbol` and
            // `&method(:name)` are neither, however well they read as a block pass.
            match named_children(*argument).as_slice() {
                [] => true,
                [only] => {
                    only.kind_str() == "identifier" && Some(context.source.node_text(*only)) == name
                }
                _ => false,
            }
        })
}

/// The name the one child of a splat, double splat or block pass spells.
fn child_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let children = named_children(node);
    let [only] = children.as_slice() else {
        return None;
    };
    (only.kind_str() == "identifier").then(|| context.source.node_text(*only))
}

/// `ruby_32_only_anonymous_forwarding?`: both sides already spell every argument anonymously, so the
/// only thing left to say is `...`.
fn is_all_anonymous(definition: Node<'_>, send: Node<'_>, context: &RuleContext<'_>) -> bool {
    if has_block_ancestor(send, context) {
        return false;
    }
    // `(args ... (restarg) (kwrestarg) (blockarg nil?))`
    let parameters = definition
        .field("parameters")
        .map(named_children)
        .unwrap_or_default();
    let anonymous_tail = match parameters.as_slice() {
        [.., rest, kwrest, block] => {
            is_anonymous(*rest, "splat_parameter")
                && is_anonymous(*kwrest, "hash_splat_parameter")
                && is_anonymous(*block, "block_parameter")
        }
        _ => false,
    };
    if !anonymous_tail {
        return false;
    }
    // `... (forwarded_restarg) (hash (forwarded_kwrestarg)) (block_pass nil?)`
    let list = grouped(send);
    match list.as_slice() {
        [.., rest, kwrest, block] => {
            rest.first()
                .is_some_and(|rest| is_anonymous(*rest, "splat_argument"))
                && kwrest.len() == 1
                && kwrest
                    .first()
                    .is_some_and(|kwrest| is_anonymous(*kwrest, "hash_splat_argument"))
                && block
                    .first()
                    .is_some_and(|block| is_anonymous(*block, "block_argument"))
        }
        _ => false,
    }
}

/// A splat, double splat or block pass written without a name.
fn is_anonymous(node: Node<'_>, kind: &str) -> bool {
    node.kind_str() == kind && named_children(node).is_empty()
}

fn has_block_ancestor(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = Some(node);
    while let Some(node) = current {
        // The `do ... end` or `{ }` written on a call is a node of its own wrapped around that call
        // upstream, so everything the call holds -- its receiver and its arguments alike -- sits
        // inside the block. Here the block is written inside the call instead, which is why the walk
        // asks each node whether it carries one rather than only looking at what it sits in.
        if BLOCKS.contains(&node.kind_str()) || node.field("block").is_some() {
            return true;
        }
        current = node.parent_of(context);
    }
    false
}

/// `can_forward_all?`.
fn can_forward_all(
    definition: Node<'_>,
    list: &[Vec<Node<'_>>],
    forwardable: &Forwardable<'_>,
    referenced: &[String],
    forwarded: (Option<Node<'_>>, Option<Node<'_>>, Option<Node<'_>>),
    settings: &Settings,
    context: &RuleContext<'_>,
) -> bool {
    let (rest, kwrest, block) = forwarded;
    let parameters = definition
        .field("parameters")
        .map(named_children)
        .unwrap_or_default();
    if is_referenced(forwardable.rest, referenced, context)
        || is_referenced(forwardable.kwrest, referenced, context)
        || is_referenced(forwardable.block, referenced, context)
    {
        return false;
    }
    // `def foo(a = 41, ...)` is a syntax error before 3.1.
    if settings.ruby <= POST_SPLAT
        && parameters
            .iter()
            .any(|parameter| parameter.kind_str() == "optional_parameter")
    {
        return false;
    }
    if settings.ruby >= ANONYMOUS && !(rest.is_some() && kwrest.is_some()) {
        return false;
    }
    // `offensive_block_forwarding?`
    let block_ok = match forwardable.block {
        Some(_) => block.is_some(),
        None => !settings.allow_only_rest,
    };
    if !block_ok {
        return false;
    }
    // `additional_kwargs_or_forwarded_kwargs?`
    if parameters.iter().any(|parameter| {
        matches!(
            parameter.kind_str(),
            "keyword_parameter" | "optional_keyword_parameter"
        )
    }) {
        return false;
    }
    if let Some(kwrest) = kwrest
        && list
            .iter()
            .any(|group| group.len() > 1 && group.iter().any(|part| part.id() == kwrest.id()))
    {
        return false;
    }

    no_additional_args(&parameters, list, forwardable, (rest, kwrest))
        || (settings.ruby >= POST_SPLAT && no_post_splat_args(list, rest))
}

/// `no_additional_args?`: the definition and the call both take exactly what is being forwarded.
fn no_additional_args(
    parameters: &[Node<'_>],
    list: &[Vec<Node<'_>>],
    forwardable: &Forwardable<'_>,
    forwarded: (Option<Node<'_>>, Option<Node<'_>>),
) -> bool {
    let (rest, kwrest) = forwarded;
    let count = [forwardable.rest, forwardable.kwrest, forwardable.block]
        .into_iter()
        .flatten()
        .count();
    // `missing_rest_arg_or_kwrest_arg?`
    if (forwardable.rest.is_some() && rest.is_none())
        || (forwardable.kwrest.is_some() && kwrest.is_none())
    {
        return false;
    }
    parameters.len() == count && list.len() == count
}

/// `no_post_splat_args?`: nothing but keywords and a block follows the splat being forwarded.
fn no_post_splat_args(list: &[Vec<Node<'_>>], rest: Option<Node<'_>>) -> bool {
    let Some(rest) = rest else {
        return true;
    };
    let Some(index) = list
        .iter()
        .position(|group| group.first().is_some_and(|first| first.id() == rest.id()))
    else {
        return true;
    };
    match list.get(index + 1).and_then(|group| group.first()) {
        None => true,
        Some(next) => matches!(
            next.kind_str(),
            "pair" | "hash" | "hash_splat_argument" | "block_argument"
        ),
    }
}

/// `add_forward_all_offenses`.
fn forward_all_offenses(
    definition: Node<'_>,
    parameters: Option<Node<'_>>,
    classified: &[Classified<'_>],
    forwardable: &Forwardable<'_>,
    settings: &Settings,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let mut registered_block = false;
    for entry in classified {
        if entry.rest.is_none()
            && entry.kwrest.is_none()
            && entry.kind != Classification::AllAnonymous
        {
            if allow_in_block(entry.block, settings, context) {
                offenses.extend(block_offense(
                    true,
                    parameters,
                    forwardable.block,
                    settings,
                    context,
                ));
                offenses.extend(block_offense(
                    true,
                    Some(entry.send),
                    entry.block,
                    settings,
                    context,
                ));
            }
            registered_block = true;
            break;
        }
        let first = entry
            .rest
            .or(entry.kwrest)
            .or_else(|| last_anonymous_splat(entry.send));
        if let Some(first) = first {
            offenses.push(all_offense(entry.send, entry.send, first, context));
        }
    }
    if registered_block {
        return;
    }
    if let (Some(parameters), Some(first)) = (parameters, forwardable.rest.or(forwardable.kwrest)) {
        offenses.push(all_offense(definition, parameters, first, context));
    }
}

/// `forward_all_first_argument`: the last anonymous splat the call was written with.
fn last_anonymous_splat<'tree>(send: Node<'tree>) -> Option<Node<'tree>> {
    grouped(send)
        .into_iter()
        .filter_map(|group| group.first().copied())
        .rfind(|argument| is_anonymous(*argument, "splat_argument"))
}

/// `add_post_ruby_32_offenses`.
fn anonymous_offenses(
    parameters: Option<Node<'_>>,
    classified: &[Classified<'_>],
    forwardable: &Forwardable<'_>,
    settings: &Settings,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    if !settings.use_anonymous {
        return;
    }
    // `all_forwarding_offenses_correctable?`: forwarding an anonymous argument out of a block was a
    // syntax error before 3.4, so a definition that does it anywhere is left alone whole.
    if settings.ruby < IN_BLOCK
        && classified
            .iter()
            .any(|entry| has_block_ancestor(entry.send, context))
    {
        return;
    }
    for entry in classified {
        if allow_in_block(entry.rest, settings, context) {
            offenses.extend(args_offense(parameters, forwardable.rest, context));
            offenses.extend(args_offense(Some(entry.send), entry.rest, context));
        }
        if allow_in_block(entry.kwrest, settings, context) {
            let parens = entry.rest.is_none();
            offenses.extend(kwargs_offense(
                parens,
                parameters,
                forwardable.kwrest,
                context,
            ));
            offenses.extend(kwargs_offense(
                parens,
                Some(entry.send),
                entry.kwrest,
                context,
            ));
        }
        if allow_in_block(entry.block, settings, context) {
            let parens = entry.rest.is_none();
            offenses.extend(block_offense(
                parens,
                parameters,
                forwardable.block,
                settings,
                context,
            ));
            offenses.extend(block_offense(
                parens,
                Some(entry.send),
                entry.block,
                settings,
                context,
            ));
        }
    }
}

/// `allow_anonymous_forwarding_in_block?`.
fn allow_in_block(node: Option<Node<'_>>, settings: &Settings, context: &RuleContext<'_>) -> bool {
    let Some(node) = node else {
        return false;
    };
    settings.ruby >= IN_BLOCK || !has_block_ancestor(node, context)
}

/// `register_forward_args_offense`.
fn args_offense(
    holder: Option<Node<'_>>,
    node: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    let (holder, node) = (holder?, node?);
    Some(replacing(node, "*", ARGS_MSG, Some(holder), context))
}

/// `register_forward_kwargs_offense`.
fn kwargs_offense(
    parens: bool,
    holder: Option<Node<'_>>,
    node: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    let node = node?;
    let holder = parens.then_some(holder).flatten();
    Some(replacing(node, "**", KWARGS_MSG, holder, context))
}

/// `register_forward_block_arg_offense`.
fn block_offense(
    parens: bool,
    holder: Option<Node<'_>>,
    node: Option<Node<'_>>,
    settings: &Settings,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    if settings.ruby < BLOCK_FORWARDING || settings.explicit_block_name {
        return None;
    }
    let node = node?;
    if context.source.node_text(node) == "&" {
        return None;
    }
    let holder = parens.then_some(holder).flatten();
    Some(replacing(node, "&", BLOCK_MSG, holder, context))
}

/// One anonymous-forwarding offense: the argument is replaced by the bare keyword, and the list it
/// sits in gains parentheses where it had none.
fn replacing(
    node: Node<'_>,
    keyword: &str,
    message: &'static str,
    holder: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Offense {
    let mut edits = holder
        .map(|holder| parenthesize(holder, context))
        .unwrap_or_default();
    edits.push(Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: keyword.to_owned(),
        safe: true,
    });
    context
        .offense(message, node.byte_range())
        .corrected_by_all(edits)
}

/// `register_forward_all_offense`: everything from the forwarded argument to the last one becomes
/// `...`.
fn all_offense(
    node: Node<'_>,
    holder: Node<'_>,
    first: Node<'_>,
    context: &RuleContext<'_>,
) -> Offense {
    let range = arguments_range(node, first);
    let mut edits = parenthesize(holder, context);
    edits.push(Edit {
        start: range.start,
        end: range.end,
        replacement: "...".to_owned(),
        safe: true,
    });
    context
        .offense(FORWARDING_MSG, range)
        .corrected_by_all(edits)
}

/// `arguments_range`: from the forwarded argument to the end of the last one the node takes.
fn arguments_range(node: Node<'_>, first: Node<'_>) -> Range<usize> {
    let last = match node.kind_str() {
        "method" | "singleton_method" => node
            .field("parameters")
            .map(named_children)
            .unwrap_or_default()
            .last()
            .copied(),
        _ => grouped(node).last().and_then(|group| group.last().copied()),
    };
    let end = last.map_or(first.end_byte(), |last| last.end_byte());
    first.start_byte()..end.max(first.end_byte())
}

/// `add_parens_if_missing`: an argument list written without parentheses gets them, since `...` and
/// the anonymous forms cannot be read without.
fn parenthesize(node: Node<'_>, context: &RuleContext<'_>) -> Vec<Edit> {
    let text = context.source.text();
    // A parameter list is a node of its own, and the space in front of it becomes the parenthesis.
    if node.kind_str() == "method_parameters" {
        if text[node.byte_range()].starts_with('(') {
            return Vec::new();
        }
        let leading = super::ranges::extended_left(text, node.start_byte(), true);
        return vec![
            Edit {
                start: leading,
                end: node.start_byte(),
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: node.end_byte(),
                end: node.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            },
        ];
    }
    // An index read is written with brackets and needs nothing.
    if node.kind_str() == "element_reference" {
        return Vec::new();
    }
    let Some(list) = node.field("arguments") else {
        return Vec::new();
    };
    if text[list.byte_range()].starts_with('(') {
        return Vec::new();
    }
    let leading = super::ranges::extended_left(text, list.start_byte(), true);
    vec![
        Edit {
            start: leading,
            end: list.start_byte(),
            replacement: "(".to_owned(),
            safe: true,
        },
        Edit {
            start: list.end_byte(),
            end: list.end_byte(),
            replacement: ")".to_owned(),
            safe: true,
        },
    ]
}
