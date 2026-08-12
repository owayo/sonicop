//! `Layout/AssignmentIndentation`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    alignment_corrections, begins_its_line, display_column, holds_block_comment, string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Indent the first line of the right-hand-side of a multi-line assignment.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let mut reported: Vec<Range<usize>> = Vec::new();

    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        let Some(right) = node.child_by_field_name("right") else {
            continue;
        };
        let Some(operator) = operator_of(node) else {
            continue;
        };
        // `same_line?(node.loc.operator, rhs)`: only a right-hand side pushed onto its own line.
        if operator.start_position().row == right.start_position().row {
            continue;
        }
        if !begins_its_line(context, right.start_byte()) {
            continue;
        }
        // `leftmost_multiple_assignment` climbs exactly one level: it recurses but throws the
        // result away and hands back the parent.
        let anchor = node
            .parent()
            .filter(|parent| {
                matches!(parent.kind(), "assignment" | "operator_assignment")
                    && parent.start_position().row == node.start_position().row
            })
            .unwrap_or(node);
        let delta = display_column(context, anchor.start_byte()) + width
            - display_column(context, right.start_byte());
        if delta == 0 {
            continue;
        }
        let expr = right.byte_range();
        let nested = reported
            .iter()
            .any(|outer| expr.start >= outer.start && expr.end <= outer.end);
        let mut offense = context.offense(MSG, expr.clone());
        if !nested && !holds_block_comment(context, &expr) {
            let taboo = string_interiors(context, &expr);
            offense = offense.corrected_by_all(alignment_corrections(
                context,
                expr.clone(),
                delta,
                &taboo,
            ));
        }
        reported.push(expr);
        offenses.push(offense);
    }
}

/// `node.loc.operator`: the `=` or the compound operator an assignment was written with.
fn operator_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let left = node.child_by_field_name("left")?;
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.start_byte() >= left.end_byte())
}
