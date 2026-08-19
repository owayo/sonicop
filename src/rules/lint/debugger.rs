use std::collections::BTreeMap;

use serde::Deserialize;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, is_string, send_range, string_text};

use super::blocks::BLOCK_KINDS;
use super::literals::literal_type;
use super::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// The configuration shape both `DebuggerMethods` and `DebuggerRequires` accept: a flat list, or
/// groups of lists that a user's configuration can switch off one at a time by setting one to `~`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Configured {
    List(Vec<String>),
    Groups(BTreeMap<String, Option<Vec<String>>>),
}

impl Configured {
    fn names(self) -> Vec<String> {
        match self {
            Self::List(names) => names,
            Self::Groups(groups) => groups.into_values().flatten().flatten().collect(),
        }
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let methods = setting(context, "DebuggerMethods");
    let requires = setting(context, "DebuggerRequires");
    if methods.is_empty() && requires.is_empty() {
        return;
    }
    // The selector each configured entry point ends in. Every `call` and every `identifier` in the
    // file reaches this cop, and the full check builds the dotted name a chain spells -- one
    // `String` per node. Comparing the last segment first costs a `str` comparison and rejects
    // everything that was never a candidate, which on ordinary code is all of it.
    let tails: Vec<&str> = methods
        .iter()
        .map(|name| name.rsplit('.').next().unwrap_or(name))
        .collect();
    // `LocalVariables` walks the file to find what each bare name means, which is only needed once
    // a name has matched.
    let mut locals: Option<LocalVariables<'_, '_>> = None;
    for node in context.nodes_of_any(&["call", "identifier"]) {
        if !selector_could_match(node, &tails, !requires.is_empty(), context) {
            continue;
        }
        if node.kind_str() == "identifier"
            && !is_receiverless_name(
                node,
                locals.get_or_insert_with(|| LocalVariables::new(context)),
            )
        {
            continue;
        }
        if node.kind_str() == "call" && !is_plain_send(node, context) {
            continue;
        }
        if !is_debugger_method(node, &methods, context)
            && !is_debugger_require(node, &requires, context)
        {
            continue;
        }
        if assumed_usage_context(node, context) {
            continue;
        }
        let range = match node.kind_str() {
            "call" => send_range(node, context),
            _ => node.byte_range(),
        };
        offenses.push(context.offense(
            format!(
                "Remove debugger entry point `{}`.",
                context.source.slice(range.clone())
            ),
            range,
        ));
    }
}

fn setting(context: &RuleContext<'_>, key: &str) -> Vec<String> {
    context
        .setting::<Configured>(key)
        .map(Configured::names)
        .unwrap_or_default()
}

/// Whether the identifier stands where upstream's parser would have built `(send nil :name)`.
fn is_receiverless_name(node: Node<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    // A name that names an argument, a hash key or the method of a call is no call of its own.
    let Some(parent) = node.parent() else {
        return false;
    };
    if matches!(
        parent.kind_str(),
        "call" | "method" | "singleton_method" | "assignment" | "operator_assignment"
    ) && parent
        .field("method")
        .or_else(|| parent.field("name"))
        .or_else(|| parent.field("left"))
        .is_some_and(|named| named.id() == node.id())
    {
        return false;
    }
    !matches!(
        parent.kind_str(),
        "block_parameters" | "method_parameters" | "lambda_parameters" | "keyword_parameter"
    ) && !locals.is_lvar(node)
}

/// Whether the name this node ends in could belong to a configured entry point at all. A `require`
/// is kept as well, since `DebuggerRequires` matches on the argument rather than on the selector.
fn selector_could_match(
    node: Node<'_>,
    tails: &[&str],
    has_requires: bool,
    context: &RuleContext<'_>,
) -> bool {
    let selector = match node.kind_str() {
        "identifier" => context.source.node_text(node),
        _ => match node.field("method") {
            Some(method) => context.source.node_text(method),
            None => return false,
        },
    };
    (has_requires && selector == "require") || tails.contains(&selector)
}

/// `debugger_method?`: the chained name the call spells is one of the configured entry points.
fn is_debugger_method(node: Node<'_>, methods: &[String], context: &RuleContext<'_>) -> bool {
    let Some(name) = chained_method_name(node, context) else {
        return false;
    };
    methods.iter().any(|method| method == &name)
}

/// `chained_method_name`: every receiver's own name, joined with dots in front of the selector.
fn chained_method_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    if node.kind_str() == "identifier" {
        return Some(context.source.node_text(node).to_owned());
    }
    let mut name = context.source.node_text(node.field("method")?).to_owned();
    let mut receiver = node.field("receiver");
    while let Some(current) = receiver {
        let part = match current.kind_str() {
            "call" => current.field("method")?,
            // `const_name` for anything that is not a send, which is only a constant here.
            _ => current,
        };
        name = format!("{}.{name}", context.source.node_text(part));
        receiver = (current.kind_str() == "call")
            .then(|| current.field("receiver"))
            .flatten();
    }
    Some(name)
}

/// `debugger_require?`: `require 'debug/start'` and the rest of the configured files.
fn is_debugger_require(node: Node<'_>, requires: &[String], context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" || node.field("receiver").is_some() {
        return false;
    }
    if node
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "require")
    {
        return false;
    }
    let call_arguments = arguments(node);
    let [feature] = call_arguments.as_slice() else {
        return false;
    };
    let feature = feature.first();
    if feature.kind_str() == "identifier" || !is_string(feature, context) {
        return false;
    }
    let value = string_text(feature, context);
    requires.iter().any(|required| required == value)
}

/// `assumed_usage_context?`: a bare entry point standing where a value is expected is far more
/// likely to be a name than a call, unless a block or a `begin` around it says otherwise.
fn assumed_usage_context(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "call" && !arguments(node).is_empty() {
        return false;
    }
    if !has_call_ancestor(node) {
        return false;
    }
    if is_assumed_argument(node, context) {
        return true;
    }
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if BLOCK_KINDS.contains(&ancestor.kind_str())
            || matches!(ancestor.kind_str(), "lambda" | "begin")
            || is_proc_or_lambda(ancestor, context)
        {
            return false;
        }
        current = ancestor.parent_of(context);
    }
    true
}

/// `each_ancestor(:call)`: every operator is a call upstream, as are an index and a plain call.
fn has_call_ancestor(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if matches!(
            ancestor.kind_str(),
            "call" | "binary" | "unary" | "element_reference"
        ) {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

/// `assumed_argument?`: `parent.call_type? || parent.literal? || parent.pair_type?`.
fn is_assumed_argument(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = upstream_parent(node) else {
        return false;
    };
    matches!(
        parent.kind_str(),
        "call" | "binary" | "unary" | "element_reference" | "pair"
    ) || literal_type(parent, context).is_some()
}

/// The node upstream would call the parent: an argument list is no node of its own there.
fn upstream_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    match parent.kind_str() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}

/// `lambda_or_proc?`: a block passed to `lambda` or `proc`.
fn is_proc_or_lambda(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    BLOCK_KINDS.contains(&node.kind_str())
        && node.parent_of(context).is_some_and(|call| {
            call.kind_str() == "call"
                && call.field("receiver").is_none()
                && call.field("method").is_some_and(|method| {
                    matches!(context.source.node_text(method), "lambda" | "proc")
                })
        })
}
