use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedReceivers").unwrap_or_default();
    for node in context.nodes_of("call") {
        if node.field("block").is_none() {
            continue;
        }
        let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        let name = context.source.node_text(method);
        // `(call !nil? :fetch _ _)` and `(send (const _ :Array) :new _ _)`.
        let accepted = match name {
            "fetch" => true,
            "new" => is_array_constant(receiver, context),
            _ => false,
        };
        if !accepted {
            continue;
        }
        let call_arguments = arguments(node);
        let [previous, default] = call_arguments.as_slice() else {
            continue;
        };
        if allowed
            .iter()
            .any(|allowed| allowed == &receiver_name(receiver, context))
        {
            continue;
        }
        // `hash_without_braces?`: the keyword arguments upstream folds into one `hash` are the
        // call's own options rather than a default value.
        if default
            .parts()
            .iter()
            .all(|part| matches!(part.kind_str(), "pair" | "hash_splat_argument"))
        {
            continue;
        }
        let range = default.range();
        offenses.push(
            context
                .offense("Block supersedes default value argument.", range.clone())
                .corrected_by(Edit {
                    start: previous.range().end,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `(const _ :Array)`: an `Array` reached through any namespace, or none.
fn is_array_constant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == "Array",
        "scope_resolution" => node
            .field("name")
            .is_some_and(|name| context.source.node_text(name) == "Array"),
        _ => false,
    }
}

/// `AllowedReceivers#receiver_name`: the chain of names in front of the call, stopping at the
/// first constant.
fn receiver_name(node: Node<'_>, context: &RuleContext<'_>) -> String {
    if node.kind_str() == "call" {
        let inner = node.field("receiver");
        if inner.is_some_and(|inner| !is_constant(inner)) {
            return receiver_name(inner.expect("checked to be present"), context);
        }
        let Some(method) = node.field("method") else {
            return context.source.node_text(node).to_owned();
        };
        return match inner {
            Some(inner) => format!(
                "{}.{}",
                receiver_name(inner, context),
                context.source.node_text(method)
            ),
            None => context.source.node_text(method).to_owned(),
        };
    }
    context.source.node_text(node).to_owned()
}

fn is_constant(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "constant" | "scope_resolution")
}
