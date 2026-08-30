use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Redundant assignment before returning detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(body) = node.field("body") else {
            continue;
        };
        check_container(context, body, offenses);
    }
}

/// A statement list, read as the `begin` upstream folds it into.
///
/// A body guarded by an `ensure` is an `ensure` node upstream, and `check_ensure_node` follows
/// `EnsureNode#branch` -- the *ensure* clause -- so the code it guards is never looked at. A body
/// split by a `rescue` alone is a `rescue` node, every branch of which is followed.
fn check_container(context: &RuleContext<'_>, container: Node<'_>, offenses: &mut Vec<Offense>) {
    let children = super::nodes::children_in(container, context);
    if let Some(ensure) = children.iter().find(|child| child.kind_str() == "ensure") {
        let statements = super::nodes::children_in(*ensure, context);
        check_statements(context, &statements, offenses);
        return;
    }
    let statements: Vec<Node<'_>> = children
        .iter()
        .copied()
        .filter(|child| !matches!(child.kind_str(), "rescue" | "else"))
        .collect();
    check_statements(context, &statements, offenses);
    if !children.iter().any(|child| child.kind_str() == "rescue") {
        return;
    }
    for clause in &children {
        match clause.kind_str() {
            "rescue" => {
                if let Some(body) = clause.field("body") {
                    check_container(context, body, offenses);
                }
            }
            "else" => {
                let statements = super::nodes::children_in(*clause, context);
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
    if assignment.kind_str() != "assignment" || reference.kind_str() != "identifier" {
        return None;
    }
    let name = assignment.field("left")?;
    if name.kind_str() != "identifier"
        || context.source.node_text(name) != context.source.node_text(reference)
    {
        return None;
    }
    assignment.field("right")
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
    match node.kind_str() {
        "case" | "case_match" => {
            for child in super::nodes::children_in(node, context) {
                match child.kind_str() {
                    "when" | "in_clause" => {
                        if let Some(body) = child.field("body") {
                            check_container(context, body, offenses);
                        }
                    }
                    "else" => {
                        let statements = super::nodes::children_in(child, context);
                        check_statements(context, &statements, offenses);
                    }
                    _ => {}
                }
            }
        }
        // A modifier form and a ternary have no branch to hold two statements.
        "if" | "unless" | "elsif" => {
            for field in ["consequence", "alternative"] {
                let Some(branch) = node.field(field) else {
                    continue;
                };
                match branch.kind_str() {
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
