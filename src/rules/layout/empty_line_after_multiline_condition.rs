//! `Layout/EmptyLineAfterMultilineCondition`: a blank line under a condition spread over lines.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use empty line after multiline condition.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&[
        "if",
        "unless",
        "elsif",
        "if_modifier",
        "unless_modifier",
        "while",
        "until",
        "while_modifier",
        "until_modifier",
    ]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        // A modifier only counts when something follows it: `on_while_post` and the modifier arm
        // of `on_if` both ask for a right sibling first.
        let modifier = node.kind_str().ends_with("_modifier");
        if modifier && next_statement(node).is_none() {
            continue;
        }
        report(context, condition, condition.byte_range(), offenses);
    }
    for node in context.nodes_of_any(&["case", "case_match"]) {
        let mut cursor = node.walk();
        for branch in node.named_children(&mut cursor) {
            if !matches!(branch.kind_str(), "when" | "in_clause") {
                continue;
            }
            let conditions = patterns(branch);
            let (Some(first), Some(last)) = (conditions.first(), conditions.last()) else {
                continue;
            };
            // `multiline_when_condition?` measures the whole list rather than one condition.
            if first.start_position().row == last.end_position().row {
                continue;
            }
            // `on_case` measures the whole condition list but corrects from the last one, and it
            // does not ask whether that last condition is itself multiline.
            report_range(context, last.byte_range(), branch.byte_range(), offenses);
        }
    }
    for node in context.nodes_of("rescue") {
        let Some(list) = node.field("exceptions") else {
            continue;
        };
        let exceptions = super::element_line_breaks::literal_elements(list);
        let (Some(first), Some(last)) = (exceptions.first(), exceptions.last()) else {
            continue;
        };
        // `multiline_rescue_exceptions?` needs more than one exception to begin with.
        if exceptions.len() <= 1
            || context.source.line_column(first.start).0 == context.source.line_column(last.end).0
        {
            continue;
        }
        let range = last.clone();
        report_range(context, range, node.byte_range(), offenses);
    }
}

/// `check_condition` and the two branches that report on something wider than what they measure.
fn report(
    context: &RuleContext<'_>,
    measured: Node<'_>,
    reported: Range<usize>,
    offenses: &mut Vec<Offense>,
) {
    if measured.start_position().row == measured.end_position().row {
        return;
    }
    report_range(context, measured.byte_range(), reported, offenses);
}

fn report_range(
    context: &RuleContext<'_>,
    measured: Range<usize>,
    reported: Range<usize>,
    offenses: &mut Vec<Offense>,
) {
    let last_line = context.source.line_column(measured.end).0;
    // `next_line_empty?`: `processed_source[line]` reads the line *after* the one numbered `line`.
    if context.source.line(last_line + 1).trim().is_empty() {
        return;
    }
    // `range_by_whole_lines`: the insertion lands at the end of the condition's last line.
    let at = context.source.line_range(last_line).end.saturating_sub(1);
    offenses.push(
        context
            .offense(MSG, reported)
            .corrections_anchored_at(at..at)
            .corrected_by(Edit {
                start: at,
                end: at,
                replacement: "\n".to_owned(),
                safe: true,
            }),
    );
}

/// `when_node.conditions`, which the grammar wraps one level deeper.
fn patterns<'tree>(branch: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = branch.walk();
    branch
        .named_children(&mut cursor)
        .filter(|child| matches!(child.kind_str(), "pattern" | "alternative_pattern"))
        .collect()
}

/// `node.right_sibling`.
fn next_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let container = node.parent()?;
    let mut cursor = container.walk();
    let statements: Vec<Node<'tree>> = container
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect();
    let position = statements
        .iter()
        .position(|statement| statement.id() == node.id())?;
    statements.get(position + 1).copied()
}
