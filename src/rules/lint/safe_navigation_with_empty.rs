use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Avoid calling `empty?` with the safe navigation operator in conditionals.";

/// The nodes upstream builds an `if` from, all of which `on_if` reaches.
const CONDITIONALS: &[&str] = &[
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(CONDITIONALS) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        // `(csend !csend :empty?)`: the safe call has no arguments, and what it is called on is a
        // plain expression rather than another safe call.
        if !is_safe_call(condition, context) || !arguments(condition).is_empty() {
            continue;
        }
        if condition
            .field("method")
            .is_none_or(|method| context.source.node_text(method) != "empty?")
        {
            continue;
        }
        let Some(receiver) = condition.field("receiver") else {
            continue;
        };
        if is_safe_call(receiver, context) {
            continue;
        }
        let source = context.source.node_text(receiver);
        offenses.push(
            context
                .offense(MSG, condition.byte_range())
                .corrected_by(Edit {
                    start: condition.start_byte(),
                    end: condition.end_byte(),
                    replacement: format!("{source} && {source}.empty?"),
                    safe: true,
                }),
        );
    }
}

fn is_safe_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && node
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "&.")
}
