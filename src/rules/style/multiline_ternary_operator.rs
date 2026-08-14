//! `Style/MultilineTernaryOperator`: a ternary spread over lines is an `if` written badly.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_IF: &str = "Avoid multi-line ternary operators, use `if` or `unless` instead.";
const MSG_SINGLE_LINE: &str = "Avoid multi-line ternary operators, use single-line instead.";

/// `COMPARISON_OPERATORS`, which `assignment_method?` exempts from its `=` test.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

/// Operators the grammar writes as `binary` and upstream as a `send`. The logical ones become
/// `and` / `or` nodes there and are not calls at all.
const LOGICAL_OPERATORS: &[&str] = &["&&", "||", "and", "or"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `ignore_node`: a ternary written inside one already being rewritten waits for the next pass.
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of("conditional") {
        let range = node.byte_range();
        if context.source.line_column(range.start).0 == context.source.line_column(range.end).0 {
            continue;
        }
        let Some(replacement) = replacement(context, node) else {
            continue;
        };
        if context.source.node_text(node) == replacement {
            continue;
        }
        let message = match single_line_wanted(context, node) {
            true => MSG_SINGLE_LINE,
            false => MSG_IF,
        };
        let offense = context.offense(message, range.clone());
        let nested = ignored
            .iter()
            .any(|outer| outer.start <= range.start && range.end <= outer.end);
        offenses.push(match nested {
            true => offense,
            false => {
                ignored.push(range.clone());
                correct(context, node, replacement, offense)
            }
        });
    }
}

fn correct(
    context: &RuleContext<'_>,
    node: Node<'_>,
    replacement: String,
    offense: Offense,
) -> Offense {
    let mut edits = vec![Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        safe: true,
    }];
    // A comment written into the condition has nowhere to go in the rewrite, so it moves above the
    // statement the ternary stood in.
    let Some(parent) = node.parent_of(context) else {
        return offense.corrected_by_all(edits);
    };
    let comments = comments_in_condition(context, node);
    if comments.is_empty() {
        return offense.corrected_by_all(edits);
    }
    edits.push(Edit {
        start: parent.start_byte(),
        end: parent.start_byte(),
        replacement: comments,
        safe: true,
    });
    offense
        .corrected_by_all(edits)
        .corrections_anchored_at(parent.byte_range())
}

/// `comments_in_range`: the comments on the lines the ternary spans, up to the line its else
/// branch begins on.
fn comments_in_condition(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let first = context.source.line_column(node.start_byte()).0;
    let Some(alternative) = node.field("alternative") else {
        return String::new();
    };
    let last = context.source.line_column(alternative.start_byte()).0;
    context
        .comment_ranges()
        .iter()
        .filter(|comment| (first..last).contains(&context.source.line_column(comment.start).0))
        .map(|comment| format!("{}\n", context.source.slice(comment.clone())))
        .collect()
}

fn replacement(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let condition = context
        .source
        .node_text(node.field("condition")?);
    let consequence = context
        .source
        .node_text(node.field("consequence")?);
    let alternative = context
        .source
        .node_text(node.field("alternative")?);
    Some(match single_line_wanted(context, node) {
        true => format!("{condition} ? {consequence} : {alternative}"),
        false => format!("if {condition}\n  {consequence}\nelse\n  {alternative}\nend"),
    })
}

/// `enforce_single_line_ternary_operator?`: where the ternary stands in for a value being handed
/// on, an `if` would not fit and the ternary is only asked to fit on one line.
fn single_line_wanted(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = upstream_parent(node) else {
        return false;
    };
    match parent.kind_str() {
        "return" | "break" | "next" => true,
        // `a[i]` and `-a` are calls upstream, named after the operator.
        "element_reference" => true,
        "unary" => true,
        "binary" => parent
            .field("operator")
            .is_some_and(|operator| {
                !LOGICAL_OPERATORS.contains(&context.source.node_text(operator))
            }),
        "call" => {
            let Some(method) = parent.field("method") else {
                return false;
            };
            // `super` is a node of its own upstream, not a `send`.
            if method.kind_str() == "super" {
                return false;
            }
            // `use_assignment_method?`: `foo.bar = x` assigns, and an `if` fits on its right.
            let name = context.source.node_text(method);
            !(name.ends_with('=') && !COMPARISON_OPERATORS.contains(&name))
        }
        _ => false,
    }
}

/// The node upstream's parser makes the parent. An argument list is tree-sitter's own: upstream
/// hangs an argument off the call itself.
fn upstream_parent<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let parent = node.parent()?;
    match parent.kind_str() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}
