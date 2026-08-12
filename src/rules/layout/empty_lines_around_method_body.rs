//! `Layout/EmptyLinesAroundMethodBody`.

use tree_sitter::Node;

use super::empty_lines_around_body::{Target, body_container, body_of, check as check_body};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut targets = Vec::new();
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let container = body_container(node);
        // An endless definition has no body container either, so the `=` is what tells the two
        // apart -- a definition with an empty body still goes through the usual check.
        if is_endless(node) {
            check_endless(context, node, offenses);
            continue;
        }
        // `adjusted_first_line`: the parameter list is where the signature really ends.
        let first_line = node
            .child_by_field_name("parameters")
            .map_or(node.start_position().row, |parameters| {
                parameters.end_position().row
            })
            + 1;
        targets.push(Target {
            first_line,
            last_line: node.end_position().row + 1,
            single_line: node.start_position().row == node.end_position().row,
            body: body_of(container),
        });
    }
    // The style is not configurable: a method body never wants blank lines around it.
    check_body(context, "method", "no_empty_lines", targets, offenses);
}

fn is_endless(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == "=")
}

/// `offending_endless_method?`: the body of `def m = value` was pushed past a blank line.
fn check_endless(context: &RuleContext<'_>, node: Node<'_>, offenses: &mut Vec<Offense>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = node.walk();
    let Some(assignment) = node.children(&mut cursor).find(|child| child.kind() == "=") else {
        return;
    };
    let assignment_line = assignment.start_position().row + 1;
    // `node.body.first_line > node.loc.assignment.line + 1`.
    if body.start_position().row < assignment_line + 1 {
        return;
    }
    if !context
        .source
        .line(assignment_line + 1)
        .trim_end_matches('\n')
        .is_empty()
    {
        return;
    }
    let start = context.source.line_start(assignment_line + 1);
    let end = (start + 1).min(context.source.text().len());
    offenses.push(
        context
            .offense(
                "Extra empty line detected at method body beginning.",
                start..end,
            )
            .corrected_by(Edit {
                start,
                end,
                replacement: String::new(),
                safe: true,
            }),
    );
}
