use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::{RuleContext, walk_named};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not use empty `case` condition, instead use an `if` expression.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("case") {
        // `case_node.condition`: the subject the parser hangs off the keyword.
        if node.field("value").is_some() {
            continue;
        }
        if is_unsupported_parent(context, node) {
            continue;
        }
        let branches = super::nodes::children(node);
        let whens: Vec<Node<'_>> = branches
            .iter()
            .copied()
            .filter(|child| child.kind_str() == "when")
            .collect();
        let Some(first_when) = whens.first().copied() else {
            continue;
        };
        // `branch_bodies.any? { |body| body.return_type? || body.each_descendant.any?(...) }`.
        let bodies = whens
            .iter()
            .filter_map(|when| when.field("body"))
            .chain(branches.iter().copied().filter(|it| it.kind_str() == "else"));
        if bodies.into_iter().any(holds_return) {
            continue;
        }
        let (Some(case_keyword), Some(when_keyword)) = (node.child(0), first_when.child(0)) else {
            continue;
        };
        let case_range = case_keyword.start_byte()..when_keyword.end_byte();
        // The insertion of `keep_first_when_comment` hangs off the line the keyword opens, not off
        // the keyword the offense reports.
        let line = case_keyword.start_position().row + 1;
        let line_start = context.source.line_start(line);
        let mut edits = vec![Edit {
            start: case_range.start,
            end: case_range.end,
            replacement: "if".to_owned(),
            safe: true,
        }];
        let comments = comments_inside(context, case_range.clone(), line_start);
        if !comments.is_empty() {
            edits.push(Edit {
                start: line_start,
                end: line_start,
                replacement: comments,
                safe: true,
            });
        }
        for when in &whens[1..] {
            let Some(keyword) = when.child(0) else {
                continue;
            };
            edits.push(Edit {
                start: keyword.start_byte(),
                end: keyword.end_byte(),
                replacement: "elsif".to_owned(),
                safe: true,
            });
        }
        correct_when_conditions(context, node, &whens, &mut edits);
        offenses.push(
            context
                .offense(MSG, case_keyword.byte_range())
                .corrected_by_all(edits)
                .corrections_anchored_at(line_start..case_range.end),
        );
    }
}

/// `NOT_SUPPORTED_PARENT_TYPES`, read through the wrappers the grammar puts between a node and
/// what upstream calls its parent.
fn is_unsupported_parent(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let parent = match parent.kind_str() {
        "argument_list" => match parent.parent() {
            Some(call) => call,
            None => return false,
        },
        _ => parent,
    };
    match parent.kind_str() {
        "return" | "break" | "next" | "yield" | "super" => true,
        "call" | "unary" | "element_reference" => true,
        // `&&` and `||` are `and` / `or` upstream rather than a message send.
        "binary" => parent
            .field("operator")
            .is_some_and(|operator| {
                !matches!(
                    context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                )
            }),
        // Assigning through a method or an index is a `send` upstream, not an assignment.
        "assignment" => parent
            .field("left")
            .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference")),
        _ => false,
    }
}

fn holds_return(body: Node<'_>) -> bool {
    let mut found = false;
    walk_named(body, &mut |node| {
        found = found || node.kind_str() == "return";
    });
    found
}

/// `keep_first_when_comment`: the comments the replacement is about to swallow, re-indented to the
/// column the `case` keyword sat at.
fn comments_inside(
    context: &RuleContext<'_>,
    case_range: std::ops::Range<usize>,
    line_start: usize,
) -> String {
    let indent = " ".repeat(case_range.start - line_start);
    let first_line = context.source.line_column(case_range.start).0;
    let last_line = context.source.line_column(case_range.end).0;
    context
        .comment_ranges()
        .iter()
        .filter(|range| {
            let line = context.source.line_column(range.start).0;
            line >= first_line && line < last_line
        })
        .map(|range| format!("{indent}{}\n", context.source.slice(range.clone())))
        .collect()
}

fn correct_when_conditions(
    context: &RuleContext<'_>,
    case_node: Node<'_>,
    whens: &[Node<'_>],
    edits: &mut Vec<Edit>,
) {
    // `when_node.parent.parent`: upstream leaves the `then` alone in a file whose whole body is
    // the `case`, because the node it reaches for is not there.
    let nested = has_upstream_parent(case_node);
    for when in whens {
        let conditions: Vec<Node<'_>> = super::nodes::children(*when)
            .into_iter()
            .filter(|child| child.kind_str() == "pattern")
            .collect();
        let (Some(first), Some(last)) = (conditions.first(), conditions.last()) else {
            continue;
        };
        if nested && let Some(keyword) = then_keyword(*when) {
            edits.push(Edit {
                start: last.end_byte(),
                end: keyword.end_byte(),
                replacement: "\n".to_owned(),
                safe: true,
            });
        }
        if conditions.len() > 1 {
            let joined = conditions
                .iter()
                .map(|condition| parenthesize(context, *condition))
                .collect::<Vec<_>>()
                .join(" || ");
            edits.push(Edit {
                start: first.start_byte(),
                end: last.end_byte(),
                replacement: joined,
                safe: true,
            });
        }
    }
}

/// `when_node.then?`: the `then` keyword the body opens with, when it is spelled rather than a
/// line break or a `;`.
fn then_keyword<'tree>(when: Node<'tree>) -> Option<Node<'tree>> {
    let body = when.field("body")?;
    let keyword = body.child(0)?;
    (keyword.kind_str() == "then").then_some(keyword)
}

/// `parenthesize_condition`: anything binding looser than `||` has to keep its own parentheses
/// once the conditions are joined with one.
fn parenthesize(context: &RuleContext<'_>, condition: Node<'_>) -> String {
    let source = context.source.node_text(condition);
    let inner = super::nodes::children(condition);
    let binds_looser = match inner.as_slice() {
        [only] => match only.kind_str() {
            "if" | "unless" | "if_modifier" | "unless_modifier" | "conditional" | "range" => true,
            "binary" => only
                .field("operator")
                .is_some_and(|operator| {
                    matches!(
                        context.source.node_text(operator),
                        "&&" | "||" | "and" | "or"
                    )
                }),
            "assignment" | "operator_assignment" => true,
            _ => false,
        },
        _ => false,
    };
    match binds_looser {
        true => format!("({source})"),
        false => source.to_owned(),
    }
}

fn has_upstream_parent(node: Node<'_>) -> bool {
    match node.parent() {
        None => false,
        // A file whose only statement is the `case` has that `case` for its root node.
        Some(parent) if parent.kind_str() == "program" => super::nodes::children(parent).len() > 1,
        Some(_) => true,
    }
}
