use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Redundant assignment before returning detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        check_container(context, body, offenses);
    }
}

/// A statement list, read as the `begin` upstream folds it into. A `rescue`, an `else` and an
/// `ensure` are clauses of the list rather than statements of it, and each is followed into on its
/// own -- except the `ensure` body, which `check_ensure_node` never looks at.
fn check_container(context: &RuleContext<'_>, container: Node<'_>, offenses: &mut Vec<Offense>) {
    let children = super::nodes::children(container);
    let statements: Vec<Node<'_>> = children
        .iter()
        .copied()
        .filter(|child| !matches!(child.kind(), "rescue" | "ensure" | "else"))
        .collect();
    check_statements(context, &statements, offenses);
    for clause in &children {
        match clause.kind() {
            "rescue" => {
                if let Some(body) = clause.child_by_field_name("body") {
                    check_container(context, body, offenses);
                }
            }
            // The `else` of a `begin`/`rescue` is a branch of the `rescue` node upstream, so it is
            // followed too. A `case`'s `else` is reached through its own handler instead.
            "else" if children.iter().any(|it| it.kind() == "rescue") => {
                let statements = super::nodes::children(*clause);
                check_statements(context, &statements, offenses);
            }
            _ => {}
        }
    }
}

fn check_statements(
    context: &RuleContext<'_>,
    statements: &[Node<'_>],
    offenses: &mut Vec<Offense>,
) {
    match statements {
        [] => {}
        // One statement is no `begin` upstream, so there is nothing for the pattern to match on.
        [only] => check_branch(context, *only, offenses),
        several => check_begin(context, several, offenses),
    }
}

/// `check_begin_node`: the pattern, and the last statement when it does not match.
fn check_begin(context: &RuleContext<'_>, statements: &[Node<'_>], offenses: &mut Vec<Offense>) {
    let [.., assignment, reference] = statements else {
        return;
    };
    if let Some(value) = redundant_assignment(context, *assignment, *reference) {
        let offense = context.offense(MSG, assignment.byte_range());
        offenses.push(match comments_between(context, *assignment, *reference) {
            true => offense,
            false => offense.corrected_by_all([
                Edit {
                    start: assignment.start_byte(),
                    end: assignment.end_byte(),
                    replacement: context.source.node_text(value).to_owned(),
                    safe: true,
                },
                Edit {
                    start: reference.start_byte(),
                    end: reference.end_byte(),
                    replacement: String::new(),
                    safe: true,
                },
            ]),
        });
        return;
    }
    check_branch(context, *reference, offenses);
}

/// `(... $(lvasgn _name _expression) (lvar _name))`: the assigned value, when the statement after
/// the assignment does nothing but read it back.
fn redundant_assignment<'t>(
    context: &RuleContext<'_>,
    assignment: Node<'t>,
    reference: Node<'t>,
) -> Option<Node<'t>> {
    if assignment.kind() != "assignment" || reference.kind() != "identifier" {
        return None;
    }
    let name = assignment.child_by_field_name("left")?;
    if name.kind() != "identifier"
        || context.source.node_text(name) != context.source.node_text(reference)
    {
        return None;
    }
    assignment.child_by_field_name("right")
}

/// `comments_between_assignment_and_reference?`, which asks by line.
fn comments_between(context: &RuleContext<'_>, assignment: Node<'_>, reference: Node<'_>) -> bool {
    let lines = assignment.start_position().row..=reference.start_position().row;
    context.comment_ranges().iter().any(|comment| {
        lines.contains(
            &context
                .source
                .line_column(comment.start)
                .0
                .saturating_sub(1),
        )
    })
}

/// `check_branch`: the branches a value can come back through.
fn check_branch(context: &RuleContext<'_>, node: Node<'_>, offenses: &mut Vec<Offense>) {
    match node.kind() {
        "case" | "case_match" => {
            for child in super::nodes::children(node) {
                match child.kind() {
                    "when" | "in_clause" => {
                        if let Some(body) = child.child_by_field_name("body") {
                            check_container(context, body, offenses);
                        }
                    }
                    "else" => {
                        let statements = super::nodes::children(child);
                        check_statements(context, &statements, offenses);
                    }
                    _ => {}
                }
            }
        }
        // A modifier form and a ternary have no branch to hold two statements.
        "if" | "unless" | "elsif" => {
            for field in ["consequence", "alternative"] {
                let Some(branch) = node.child_by_field_name(field) else {
                    continue;
                };
                match branch.kind() {
                    "then" | "else" => check_container(context, branch, offenses),
                    _ => check_branch(context, branch, offenses),
                }
            }
        }
        // `begin ... end` is a `kwbegin` upstream, which holds its statements itself.
        "begin" => check_container(context, node, offenses),
        _ => {}
    }
}
