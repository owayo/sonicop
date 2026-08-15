use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 2.7`.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);
/// `def foo(a = 41, ...)` is a syntax error up to here, and a `...` may not follow a `*` either.
const RUBY_30: RubyVersion = RubyVersion::new(3, 0);
/// The version that gave `*` and `**` a meaning of their own.
const RUBY_32: RubyVersion = RubyVersion::new(3, 2);
/// The version that let an anonymous argument be forwarded from inside a block.
const RUBY_34: RubyVersion = RubyVersion::new(3, 4);

const FORWARDING_MSG: &str = "Use shorthand syntax `...` for arguments forwarding.";
const ARGS_MSG: &str = "Use anonymous positional arguments forwarding (`*`).";
const KWARGS_MSG: &str = "Use anonymous keyword arguments forwarding (`**`).";
const BLOCK_MSG: &str = "Use anonymous block arguments forwarding (`&`).";

/// A definition that takes arguments only to hand them straight on, which `...` says in one.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let settings = Settings::new(context);
    let locals = LocalVariables::new(context);
    for def_node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(body) = def_node.field("body") else {
            continue;
        };
        let parameters = def_node.field("parameters");
        let written = parameters.map(super::nodes::children).unwrap_or_default();
        let forwardable = Forwardable::of(&written, &settings, context);
        let referenced = referenced_names(body, context, &locals);
        let classifications: Vec<Classification<'_>> = sends(def_node)
            .into_iter()
            .filter_map(|send| {
                Classification::of(
                    def_node,
                    &written,
                    send,
                    &referenced,
                    &forwardable,
                    &settings,
                    context,
                )
            })
            .collect();
        if classifications.is_empty() {
            continue;
        }
        if classifications
            .iter()
            .all(|found| matches!(found.kind, Kind::All | Kind::AllAnonymous))
        {
            forward_all_offenses(
                def_node,
                parameters,
                &written,
                &classifications,
                &forwardable,
                &settings,
                context,
                offenses,
            );
        } else if settings.target >= RUBY_32 {
            anonymous_offenses(
                parameters,
                &classifications,
                &forwardable,
                &settings,
                context,
                offenses,
            );
        }
    }
}

/// What the configuration says, gathered once.
struct Settings {
    target: RubyVersion,
    allow_only_rest_arguments: bool,
    use_anonymous_forwarding: bool,
    rest_names: Vec<String>,
    kwrest_names: Vec<String>,
    block_names: Vec<String>,
    explicit_block_name: bool,
}

impl Settings {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            target: context.target_ruby_version(),
            allow_only_rest_arguments: context.setting("AllowOnlyRestArgument").unwrap_or(true),
            use_anonymous_forwarding: context.setting("UseAnonymousForwarding").unwrap_or(false),
            rest_names: context
                .setting("RedundantRestArgumentNames")
                .unwrap_or_default(),
            kwrest_names: context
                .setting("RedundantKeywordRestArgumentNames")
                .unwrap_or_default(),
            block_names: context
                .setting("RedundantBlockArgumentNames")
                .unwrap_or_default(),
            // `config.for_enabled_cop('Naming/BlockForwarding')['EnforcedStyle']`.
            explicit_block_name: context.cop_enabled("Naming/BlockForwarding")
                && context
                    .setting_of::<String>("Naming/BlockForwarding", "EnforcedStyle")
                    .as_deref()
                    == Some("explicit"),
        }
    }

    /// `allow_anonymous_forwarding_in_block?`.
    ///
    /// Ruby 3.3 made reading an anonymous argument inside a block a syntax error, so a forwarding
    /// written there is left alone for every version before the one that allowed it again.
    fn anonymous_in_block(&self, node: Option<Node<'_>>, context: &RuleContext<'_>) -> bool {
        let Some(node) = node else {
            return false;
        };
        self.target >= RUBY_34 || !inside_block(node, context)
    }
}

/// `redundant_forwardable_named_args`: the three parameters whose names say nothing the shorthand
/// does not.
struct Forwardable<'tree> {
    rest: Option<Node<'tree>>,
    kwrest: Option<Node<'tree>>,
    block: Option<Node<'tree>>,
}

impl<'tree> Forwardable<'tree> {
    fn of(written: &[Node<'tree>], settings: &Settings, context: &RuleContext<'_>) -> Self {
        let find = |kind: &str| written.iter().find(|node| node.kind_str() == kind).copied();
        Self {
            rest: redundant(find("splat_parameter"), &settings.rest_names, "*", context),
            kwrest: redundant(
                find("hash_splat_parameter"),
                &settings.kwrest_names,
                "**",
                context,
            ),
            block: redundant(find("block_parameter"), &settings.block_names, "&", context),
        }
    }

    fn count(&self) -> usize {
        usize::from(self.rest.is_some())
            + usize::from(self.kwrest.is_some())
            + usize::from(self.block.is_some())
    }
}

/// `redundant_named_arg`: the parameter, but only when it is written as the keyword alone or with
/// one of the names the configuration calls redundant.
fn redundant<'tree>(
    parameter: Option<Node<'tree>>,
    names: &[String],
    keyword: &str,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let parameter = parameter?;
    let source = context.source.node_text(parameter);
    (source == keyword
        || names
            .iter()
            .any(|name| format!("{keyword}{name}") == source))
    .then_some(parameter)
}

/// `classification`.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    All,
    AllAnonymous,
    RestOrKwrest,
}

/// One call inside the definition, with what it forwards.
struct Classification<'tree> {
    send: Node<'tree>,
    kind: Kind,
    rest: Option<Node<'tree>>,
    kwrest: Option<Node<'tree>>,
    block: Option<Node<'tree>>,
}

impl<'tree> Classification<'tree> {
    fn of(
        def_node: Node<'tree>,
        written: &[Node<'tree>],
        send: Node<'tree>,
        referenced: &[String],
        forwardable: &Forwardable<'tree>,
        settings: &Settings,
        context: &'tree RuleContext<'_>,
    ) -> Option<Self> {
        let name = |node: Option<Node<'_>>| {
            node.and_then(|node| node.field("name"))
                .map(|name| context.source.node_text(name).to_owned())
        };
        let (rest_name, kwrest_name, block_name) = (
            name(forwardable.rest),
            name(forwardable.kwrest),
            name(forwardable.block),
        );
        let is_referenced =
            |name: &Option<String>| name.as_ref().is_some_and(|name| referenced.contains(name));
        let arguments = upstream_arguments(send);
        // `forwarded_rest_arg` / `forwarded_kwrest_arg` / `forwarded_block_arg`.
        let rest = (!is_referenced(&rest_name))
            .then(|| {
                forwarded(
                    &arguments,
                    "splat_argument",
                    rest_name.as_deref(),
                    false,
                    context,
                )
            })
            .flatten();
        let kwrest = (!is_referenced(&kwrest_name))
            .then(|| {
                forwarded(
                    &arguments,
                    "hash_splat_argument",
                    kwrest_name.as_deref(),
                    false,
                    context,
                )
            })
            .flatten();
        let block = (!is_referenced(&block_name))
            .then(|| {
                forwarded(
                    &arguments,
                    "block_argument",
                    block_name.as_deref(),
                    true,
                    context,
                )
            })
            .flatten();
        if rest.is_none() && kwrest.is_none() && block.is_none() {
            return None;
        }
        let referenced_any =
            is_referenced(&rest_name) || is_referenced(&kwrest_name) || is_referenced(&block_name);
        let kind = if only_anonymous(def_node, written, send, &arguments, context) {
            Kind::AllAnonymous
        } else if can_forward_all(
            def_node,
            written,
            send,
            &arguments,
            referenced_any,
            (rest, kwrest, block),
            forwardable,
            settings,
            context,
        ) {
            Kind::All
        } else {
            Kind::RestOrKwrest
        };
        Some(Self {
            send,
            kind,
            rest,
            kwrest,
            block,
        })
    }
}

/// `forwarded_rest_arg?` and its siblings: the argument that hands the named parameter straight on.
///
/// An anonymous `&` matches whatever the block parameter was called, which is what `{(lvar %1)
/// nil?}` allows for and the other two patterns do not.
fn forwarded<'tree>(
    arguments: &[Argument<'tree>],
    kind: &str,
    name: Option<&str>,
    anonymous_matches: bool,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    arguments.iter().find_map(|argument| {
        argument.parts.iter().copied().find(|part| {
            if part.kind_str() != kind {
                return false;
            }
            match super::nodes::children(*part).first() {
                Some(inner) => {
                    inner.kind_str() == "identifier"
                        && Some(context.source.node_text(*inner)) == name
                }
                None => anonymous_matches,
            }
        })
    })
}

/// An argument as upstream's parser counts one: the trailing keyword arguments are one `hash` there
/// however many were written.
struct Argument<'tree> {
    /// `:hash` for the folded run, and the node's own kind otherwise.
    kind: &'static str,
    parts: Vec<Node<'tree>>,
}

fn upstream_arguments<'tree>(send: Node<'tree>) -> Vec<Argument<'tree>> {
    // An index takes its arguments in brackets rather than in a list of its own, and upstream's
    // parser reads them as the arguments of a `:[]` send all the same.
    let written = match send.kind_str() {
        "element_reference" => {
            let object = send.field("object").map(|object| object.id());
            super::nodes::children(send)
                .into_iter()
                .filter(|child| Some(child.id()) != object)
                .collect()
        }
        _ => match argument_list(send) {
            Some(list) => super::nodes::children(list),
            None => return Vec::new(),
        },
    };
    let mut arguments: Vec<Argument<'tree>> = Vec::new();
    for node in written {
        let keyword = matches!(node.kind_str(), "pair" | "hash_splat_argument");
        match arguments.last_mut() {
            Some(last) if keyword && last.kind == "hash" => last.parts.push(node),
            _ => arguments.push(Argument {
                kind: if keyword { "hash" } else { node.kind_str() },
                parts: match node.kind_str() {
                    // A braced hash holds its own pairs, which is where a `**` written inside one
                    // is looked for.
                    "hash" => super::nodes::children(node),
                    _ => vec![node],
                },
            }),
        }
    }
    arguments
}

/// `ruby_32_only_anonymous_forwarding?`.
fn only_anonymous<'tree>(
    def_node: Node<'tree>,
    written: &[Node<'tree>],
    send: Node<'tree>,
    arguments: &[Argument<'tree>],
    context: &'tree RuleContext<'_>,
) -> bool {
    if inside_block(send, context) {
        return false;
    }
    let _ = def_node;
    // `(args ... (restarg) (kwrestarg) (blockarg nil?))`.
    let anonymous = |node: Option<&Node<'_>>, kind: &str| {
        node.is_some_and(|node| node.kind_str() == kind && node.field("name").is_none())
    };
    let tail = written.len().checked_sub(3);
    let def_anonymous = tail.is_some_and(|start| {
        anonymous(written.get(start), "splat_parameter")
            && anonymous(written.get(start + 1), "hash_splat_parameter")
            && anonymous(written.get(start + 2), "block_parameter")
    });
    if !def_anonymous {
        return false;
    }
    // `... (forwarded_restarg) (hash (forwarded_kwrestarg)) (block_pass nil?)`.
    let Some(start) = arguments.len().checked_sub(3) else {
        return false;
    };
    let bare = |argument: Option<&Argument<'_>>, kind: &str| {
        argument.is_some_and(|argument| {
            matches!(argument.parts.as_slice(), [only]
                if only.kind_str() == kind && only.named_child_count() == 0)
        })
    };
    bare(arguments.get(start), "splat_argument")
        && arguments.get(start + 1).is_some_and(|argument| {
            argument.kind == "hash"
                && matches!(argument.parts.as_slice(), [only]
                    if only.kind_str() == "hash_splat_argument" && only.named_child_count() == 0)
        })
        && bare(arguments.get(start + 2), "block_argument")
}

/// `can_forward_all?`.
#[expect(
    clippy::too_many_arguments,
    reason = "upstream reads the same eight things"
)]
fn can_forward_all<'tree>(
    def_node: Node<'tree>,
    written: &[Node<'tree>],
    send: Node<'tree>,
    arguments: &[Argument<'tree>],
    referenced_any: bool,
    forwarded: (
        Option<Node<'tree>>,
        Option<Node<'tree>>,
        Option<Node<'tree>>,
    ),
    forwardable: &Forwardable<'tree>,
    settings: &Settings,
    context: &'tree RuleContext<'_>,
) -> bool {
    let (rest, kwrest, block) = forwarded;
    let _ = (def_node, send, context);
    if referenced_any {
        return false;
    }
    // `ruby_30_or_lower_optarg?`.
    if settings.target <= RUBY_30
        && written
            .iter()
            .any(|node| node.kind_str() == "optional_parameter")
    {
        return false;
    }
    // `ruby_32_or_higher_missing_rest_or_kwest?`.
    if settings.target >= RUBY_32 && !(rest.is_some() && kwrest.is_some()) {
        return false;
    }
    // `offensive_block_forwarding?`.
    let offensive_block = match forwardable.block {
        Some(_) => block.is_some(),
        None => !settings.allow_only_rest_arguments,
    };
    if !offensive_block {
        return false;
    }
    // `additional_kwargs_or_forwarded_kwargs?`.
    if written
        .iter()
        .any(|node| node.kind_str() == "keyword_parameter")
    {
        return false;
    }
    if kwrest.is_some_and(|kwrest| {
        arguments
            .iter()
            .any(|argument| argument.parts.iter().any(|part| part.id() == kwrest.id()))
            && arguments
                .iter()
                .find(|argument| argument.parts.iter().any(|part| part.id() == kwrest.id()))
                .is_some_and(|argument| argument.parts.len() != 1)
    }) {
        return false;
    }
    // `no_additional_args?`.
    let missing = (forwardable.rest.is_some() && rest.is_none())
        || (forwardable.kwrest.is_some() && kwrest.is_none());
    let count = forwardable.count();
    let no_additional = !missing && written.len() == count && arguments.len() == count;
    if no_additional {
        return true;
    }
    // `no_post_splat_args?`.
    settings.target >= RUBY_30 && no_post_splat_args(arguments, rest)
}

fn no_post_splat_args<'tree>(arguments: &[Argument<'tree>], rest: Option<Node<'tree>>) -> bool {
    let Some(rest) = rest else {
        return true;
    };
    let Some(index) = arguments
        .iter()
        .position(|argument| argument.parts.iter().any(|part| part.id() == rest.id()))
    else {
        return true;
    };
    match arguments.get(index + 1) {
        None => true,
        Some(argument) => matches!(argument.kind, "hash" | "block_argument"),
    }
}

/// `add_forward_all_offenses`.
#[expect(
    clippy::too_many_arguments,
    reason = "upstream reads the same eight things"
)]
fn forward_all_offenses<'tree>(
    def_node: Node<'tree>,
    parameters: Option<Node<'tree>>,
    written: &[Node<'tree>],
    classifications: &[Classification<'tree>],
    forwardable: &Forwardable<'tree>,
    settings: &Settings,
    context: &'tree RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let _ = def_node;
    for found in classifications {
        if found.rest.is_none() && found.kwrest.is_none() && found.kind != Kind::AllAnonymous {
            if settings.anonymous_in_block(found.block, context) {
                block_offense(
                    parameters,
                    forwardable.block,
                    true,
                    settings,
                    context,
                    offenses,
                );
                block_offense(
                    argument_list(found.send),
                    found.block,
                    true,
                    settings,
                    context,
                    offenses,
                );
            }
            return;
        }
        let first = found
            .rest
            .or(found.kwrest)
            .or_else(|| last_forwarded_restarg(found.send));
        all_offense(
            argument_list(found.send),
            upstream_arguments(found.send).last().map(Argument::end),
            first,
            context,
            offenses,
        );
    }
    all_offense(
        parameters,
        written.last().map(|node| node.end_byte()),
        forwardable.rest.or(forwardable.kwrest),
        context,
        offenses,
    );
}

/// `forward_all_first_argument`: the last `*` written among the arguments.
fn last_forwarded_restarg<'tree>(send: Node<'tree>) -> Option<Node<'tree>> {
    upstream_arguments(send).iter().rev().find_map(|argument| {
        argument
            .parts
            .iter()
            .rev()
            .find(|part| part.kind_str() == "splat_argument" && part.named_child_count() == 0)
            .copied()
    })
}

/// `add_post_ruby_32_offenses`.
fn anonymous_offenses<'tree>(
    parameters: Option<Node<'tree>>,
    classifications: &[Classification<'tree>],
    forwardable: &Forwardable<'tree>,
    settings: &Settings,
    context: &'tree RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    if !settings.use_anonymous_forwarding {
        return;
    }
    // `all_forwarding_offenses_correctable?`.
    if settings.target < RUBY_34
        && classifications
            .iter()
            .any(|found| inside_block(found.send, context))
    {
        return;
    }
    for found in classifications {
        let list = argument_list(found.send);
        if settings.anonymous_in_block(found.rest, context) {
            anonymous_offense(
                parameters,
                forwardable.rest,
                ARGS_MSG,
                "*",
                true,
                context,
                offenses,
            );
            anonymous_offense(list, found.rest, ARGS_MSG, "*", true, context, offenses);
        }
        let add_parens = found.rest.is_none();
        if settings.anonymous_in_block(found.kwrest, context) {
            anonymous_offense(
                parameters,
                forwardable.kwrest,
                KWARGS_MSG,
                "**",
                add_parens,
                context,
                offenses,
            );
            anonymous_offense(
                list,
                found.kwrest,
                KWARGS_MSG,
                "**",
                add_parens,
                context,
                offenses,
            );
        }
        if settings.anonymous_in_block(found.block, context) {
            block_offense(
                parameters,
                forwardable.block,
                add_parens,
                settings,
                context,
                offenses,
            );
            block_offense(list, found.block, add_parens, settings, context, offenses);
        }
    }
}

/// `register_forward_args_offense` and `register_forward_kwargs_offense`.
fn anonymous_offense<'tree>(
    list: Option<Node<'tree>>,
    node: Option<Node<'tree>>,
    message: &str,
    replacement: &str,
    add_parens: bool,
    context: &'tree RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(node) = node else {
        return;
    };
    let mut edits = vec![Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: replacement.to_owned(),
        safe: true,
    }];
    if add_parens {
        edits.extend(parentheses_if_missing(list, context));
    }
    offenses.push(
        context
            .offense(message, node.byte_range())
            .corrected_by_all(edits),
    );
}

/// `register_forward_block_arg_offense`.
fn block_offense<'tree>(
    list: Option<Node<'tree>>,
    node: Option<Node<'tree>>,
    add_parens: bool,
    settings: &Settings,
    context: &'tree RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    if settings.target <= RUBY_30 || settings.explicit_block_name {
        return;
    }
    let Some(node) = node.filter(|node| context.source.node_text(*node) != "&") else {
        return;
    };
    anonymous_offense(
        list,
        Some(node),
        BLOCK_MSG,
        "&",
        add_parens,
        context,
        offenses,
    );
}

/// `register_forward_all_offense`: the run from the first forwarded argument to the last one.
fn all_offense<'tree>(
    list: Option<Node<'tree>>,
    last: Option<usize>,
    first: Option<Node<'tree>>,
    context: &'tree RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let (Some(first), Some(last)) = (first, last) else {
        return;
    };
    let range = first.start_byte()..last;
    if range.end < range.start {
        return;
    }
    let mut edits = vec![Edit {
        start: range.start,
        end: range.end,
        replacement: "...".to_owned(),
        safe: true,
    }];
    edits.extend(parentheses_if_missing(list, context));
    offenses.push(
        context
            .offense(FORWARDING_MSG, range)
            .corrected_by_all(edits),
    );
}

/// `add_parens_if_missing`.
fn parentheses_if_missing<'tree>(
    list: Option<Node<'tree>>,
    context: &'tree RuleContext<'_>,
) -> Vec<Edit> {
    let Some(list) = list else {
        return Vec::new();
    };
    if context.source.node_text(list).starts_with('(') {
        return Vec::new();
    }
    // `node.method?(:[])`: an index takes its arguments in brackets already.
    if list
        .parent_of(context)
        .is_some_and(|parent| parent.kind_str() == "element_reference")
    {
        return Vec::new();
    }
    let text = context.source.text().as_bytes();
    let mut start = list.start_byte();
    while start > 0 && matches!(text[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    vec![
        Edit {
            start,
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

impl Argument<'_> {
    fn end(&self) -> usize {
        self.parts.last().map_or(0, |part| part.end_byte())
    }
}

/// `non_splat_or_block_pass_lvar_references`: every local variable the body reads for itself rather
/// than to forward.
fn referenced_names(
    body: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Vec<String> {
    let mut names = Vec::new();
    collect_references(body, context, locals, &mut names);
    names.sort_unstable();
    names.dedup();
    names
}

fn collect_references(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    names: &mut Vec<String>,
) {
    // Every name written on the left of an assignment is an `lvasgn` there, however many were
    // written and whatever the value turns out to be.
    if matches!(node.kind_str(), "assignment" | "operator_assignment")
        && let Some(left) = node.field("left")
    {
        collect_targets(left, context, names);
    }
    for child in super::nodes::children(node) {
        let forwarding = matches!(
            node.kind_str(),
            "splat_argument" | "hash_splat_argument" | "block_argument"
        );
        if child.kind_str() == "identifier" && !forwarding && locals.is_lvar(child) {
            names.push(context.source.node_text(child).to_owned());
        }
        collect_references(child, context, locals, names);
    }
}

/// The names an assignment writes to, reaching into the list a multiple assignment spreads over.
fn collect_targets(node: Node<'_>, context: &RuleContext<'_>, names: &mut Vec<String>) {
    match node.kind_str() {
        "identifier" => names.push(context.source.node_text(node).to_owned()),
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
            for child in super::nodes::children(node) {
                collect_targets(child, context, names);
            }
        }
        _ => {}
    }
}

/// `node.each_descendant(:call, :super, :yield)`.
fn sends<'tree>(def_node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    collect_sends(def_node, &mut found);
    found
}

fn collect_sends<'tree>(node: Node<'tree>, found: &mut Vec<Node<'tree>>) {
    for child in super::nodes::children(node) {
        if matches!(
            child.kind_str(),
            "call" | "super" | "yield" | "element_reference"
        ) {
            found.push(child);
        }
        collect_sends(child, found);
    }
}

/// The list the arguments were written in, which a `yield` and a `super` hold without naming.
fn argument_list<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("arguments").or_else(|| {
        super::nodes::children(node)
            .into_iter()
            .find(|child| child.kind_str() == "argument_list")
    })
}

/// `node.each_ancestor(:any_block).any?`.
fn inside_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = Some(node);
    while let Some(candidate) = current {
        if matches!(candidate.kind_str(), "block" | "do_block" | "lambda") {
            return true;
        }
        // A block is a node written *around* the call upstream rather than hung off it, so a call
        // that carries one stands inside it there -- and so does everything the call holds.
        if candidate
            .field("block")
            .is_some_and(|block| matches!(block.kind_str(), "block" | "do_block"))
        {
            return true;
        }
        current = candidate.parent_of(context);
    }
    false
}
