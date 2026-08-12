use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range, top_level_constant};

use super::node_equality::numeric_value;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(method) != "rand" || !is_plain_send(node, context) {
            continue;
        }
        // `{(const {nil? cbase} :Kernel) nil?}`: no receiver at all, or `Kernel` reached from the
        // top level.
        if node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| !top_level_constant(receiver, "Kernel", context))
        {
            continue;
        }
        let arguments = arguments(node);
        let [argument] = arguments.as_slice() else {
            continue;
        };
        if !is_one(argument.first(), context) {
            continue;
        }
        let range = send_range(node, context);
        let message = format!(
            "`{}` always returns `0`. Perhaps you meant `rand(2)` or `rand`?",
            context.source.slice(range.clone()),
        );
        offenses.push(context.offense(message, range));
    }
}

/// `{(int {-1 1}) (float {-1.0 1.0})}`. An `int` can never equal `1.0` and a `float` never `1`, so
/// the two halves together are exactly "a numeric literal worth plus or minus one" -- and it is the
/// value that decides it, `0x1` being the same `(int 1)` upstream as `1`.
fn is_one(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(node.kind(), "integer" | "float" | "unary")
        && numeric_value(node, context).is_some_and(|value| value.abs() == 1.0)
}
