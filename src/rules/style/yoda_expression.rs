use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `RESTRICT_ON_SEND = %i[* + & | ^]`, which `SupportedOperators` then narrows.
const RESTRICTED: [&str; 5] = ["*", "+", "&", "|", "^"];

/// `constant_portion?`: `node.type?(:numeric, :const)`.
const NUMERIC_KINDS: [&str; 4] = ["integer", "float", "rational", "complex"];

/// An operation with the literal on the left, which reads backwards.
///
/// Only the outermost one of a nest is reported: once an operation has been swapped, the operations
/// inside it are left to the pass that follows.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let supported = context
        .setting::<Vec<String>>("SupportedOperators")
        .unwrap_or_default();
    let mut offended: HashSet<usize> = HashSet::new();
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((operator, lhs, rhs)) = operation(node, context) else {
            continue;
        };
        if !RESTRICTED.contains(&operator) || !supported.iter().any(|name| name == operator) {
            continue;
        }
        if !is_constant(lhs) || is_constant(rhs) {
            continue;
        }
        // `offended_ancestor?`: an ancestor call that has already been swapped covers this one.
        if ancestors(node).any(|ancestor| offended.contains(&ancestor.id())) {
            continue;
        }
        offended.insert(node.id());
        let (left, right) = (
            context.source.node_text(lhs).to_owned(),
            context.source.node_text(rhs).to_owned(),
        );
        offenses.push(
            context
                .offense(
                    format!("Non-literal operand (`{right}`) should be first."),
                    send_node::send_range(node, context),
                )
                // `corrector.swap(lhs, rhs)`: two replacements, one per operand.
                .corrected_by_all([
                    Edit {
                        start: lhs.start_byte(),
                        end: lhs.end_byte(),
                        replacement: right,
                        safe: true,
                    },
                    Edit {
                        start: rhs.start_byte(),
                        end: rhs.end_byte(),
                        replacement: left,
                        safe: true,
                    },
                ]),
        );
    }
}

/// The operator and the two operands, for the infix spelling and the explicit call alike.
fn operation<'a, 'tree>(
    node: Node<'tree>,
    context: &'a RuleContext<'_>,
) -> Option<(&'a str, Node<'tree>, Node<'tree>)> {
    match node.kind_str() {
        "binary" => {
            let operator = node.field("operator")?;
            Some((
                context.source.node_text(operator),
                node.field("left")?,
                node.field("right")?,
            ))
        }
        _ => {
            let selector = node.field("method")?;
            let arguments = node.field("arguments").map(super::nodes::children)?;
            let first = arguments.first().copied()?;
            Some((
                context.source.node_text(selector),
                node.field("receiver")?,
                first,
            ))
        }
    }
}

/// `constant_portion?`: a numeric literal or a constant. A signed literal is a single `int`
/// upstream, so the sign is looked through here.
fn is_constant(node: Node<'_>) -> bool {
    let node = match node.kind_str() {
        "unary" => node.field("operand").unwrap_or(node),
        _ => node,
    };
    NUMERIC_KINDS.contains(&node.kind_str())
        || matches!(node.kind_str(), "constant" | "scope_resolution")
}

/// The ancestors of a node, outermost last.
fn ancestors<'tree>(node: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    std::iter::successors(node.parent(), |current| current.parent())
}
