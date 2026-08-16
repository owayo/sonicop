//! `StatementModifier`: what the two cops that fold a body onto its condition's line share.
//!
//! Upstream is one mixin, included by both `Style/IfUnlessModifier` and `Style/WhileUntilModifier`,
//! so a condition one of them refuses to move is refused by the other for the same reason.

use tree_sitter::Node;

use super::conditional::descendants;
use super::nodes;
use crate::rules::node_ext::NodeExt;

/// `condition.each_node.any?(&:lvasgn_type?)`: a condition that binds a local cannot move behind
/// the body that reads it.
pub(super) fn non_eligible_condition(condition: Node<'_>) -> bool {
    descendants(condition).into_iter().any(|node| {
        matches!(node.kind_str(), "assignment" | "operator_assignment")
            && node.field("left").is_some_and(binds_local)
    })
}

/// Whether the left-hand side of an assignment writes a local anywhere in it. A multiple
/// assignment is one `masgn` upstream, but every name it writes is still an `lvasgn` beneath it.
fn binds_local(left: Node<'_>) -> bool {
    match left.kind_str() {
        "identifier" => true,
        "left_assignment_list" | "rest_assignment" | "destructured_left_assignment" => {
            nodes::children(left).into_iter().any(binds_local)
        }
        _ => false,
    }
}
