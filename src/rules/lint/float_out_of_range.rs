use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Float out of range.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("float") {
        let source = context.source.node_text(node);
        // A literal too large for a `Float` reads back as an infinity, and one too small as a
        // zero -- which is only out of range when digits other than zeros were written.
        let Ok(value) = source.replace('_', "").parse::<f64>() else {
            continue;
        };
        let significant = source.chars().any(|digit| ('1'..='9').contains(&digit));
        if value.is_infinite() || (value == 0.0 && significant) {
            offenses.push(context.offense(MSG, signed(node, context).byte_range()));
        }
    }
}

/// The literal as upstream's parser built it: a sign written before a numeric literal is folded
/// into it, so the node a cop reports starts at the sign rather than at the digits.
fn signed<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Node<'tree> {
    node.parent()
        .filter(|parent| {
            parent.kind() == "unary"
                && parent
                    .child_by_field_name("operand")
                    .is_some_and(|operand| operand.id() == node.id())
                && parent
                    .child_by_field_name("operator")
                    .is_some_and(|operator| {
                        matches!(context.source.node_text(operator), "-" | "+")
                    })
        })
        .unwrap_or(node)
}
