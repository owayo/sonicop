use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((comparison, left, right)) = comparison(node, context) else {
            continue;
        };
        // `(send _lhs_receiver :object_id)` on both sides. A wildcard matches no missing receiver,
        // so a bare `object_id` never pairs with anything.
        let (Some(receiver), Some(argument)) = (
            object_id_receiver(left, context),
            object_id_receiver(right, context),
        ) else {
            continue;
        };
        let bang = if comparison == "==" { "" } else { "!" };
        let message =
            format!("Use `{bang}equal?` instead of `{comparison}` when comparing `object_id`.");
        let range = send_range(node, context);
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: format!(
                "{bang}{}.equal?({})",
                context.source.node_text(receiver),
                context.source.node_text(argument),
            ),
            safe: true,
        }));
    }
}

/// The two operands of an `==` or `!=`, which is written as a `binary` node unless a dot was put in
/// front of the operator.
fn comparison<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(&'static str, Node<'tree>, Node<'tree>)> {
    let (operator, left, right) = match node.kind_str() {
        "binary" => (
            node.field("operator")?,
            node.field("left")?,
            node.field("right")?,
        ),
        "call" if is_plain_send(node, context) => {
            let arguments = arguments(node);
            let [argument] = arguments.as_slice() else {
                return None;
            };
            (
                node.field("method")?,
                node.field("receiver")?,
                argument.first(),
            )
        }
        _ => return None,
    };
    let comparison = match context.source.node_text(operator) {
        "==" => "==",
        "!=" => "!=",
        _ => return None,
    };
    Some((comparison, left, right))
}

/// The receiver of a `(send _ :object_id)`, or `None` when the operand is anything else.
fn object_id_receiver<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "call" || !is_plain_send(node, context) {
        return None;
    }
    let method = node.field("method")?;
    if context.source.node_text(method) != "object_id" || !arguments(node).is_empty() {
        return None;
    }
    node.field("block")
        .is_none()
        .then(|| node.field("receiver"))?
}
