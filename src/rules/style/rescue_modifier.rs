//! `Style/RescueModifier`: `x rescue nil` swallows every error, so write the block out.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;

const MSG: &str = "Avoid using `rescue` in its modifier form.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width = context
        .setting_of::<usize>("Layout/IndentationWidth", "Width")
        .unwrap_or(2);
    for node in context.nodes_of("rescue_modifier") {
        let (Some(operation), Some(handler)) = (
            node.child_by_field_name("body"),
            node.child_by_field_name("handler"),
        ) else {
            continue;
        };
        // `parenthesized?`: the parentheses around the whole expression go with the rewrite.
        let parenthesized = node
            .parent()
            .filter(|parent| parent.kind() == "parenthesized_statements");
        let (indentation, offset) = indentation_and_offset(context, node, width, parenthesized);

        let mut edits = Vec::new();
        // A comma-separated list of values is one array upstream, and it needs brackets once it no
        // longer sits alone on its line.
        if operation.kind() == "right_assignment_list" {
            edits.push(insert(operation.start_byte(), "["));
            edits.push(insert(operation.end_byte(), "]"));
        }
        edits.push(Edit {
            start: operation.end_byte(),
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        });
        edits.push(insert(
            operation.start_byte(),
            format!("begin\n{indentation}"),
        ));
        let clause = format!(
            "\n{offset}rescue\n{indentation}{}\n{offset}end",
            context.source.node_text(handler)
        );
        // A heredoc opened by the operation has its body written after the whole statement, so the
        // `end` goes after the terminator rather than after the call that opened it.
        let after = heredoc_end(context, operation).unwrap_or_else(|| operation.end_byte());
        edits.push(insert(after, clause));
        if let Some(parenthesized) = parenthesized {
            edits.extend(super::parens::correct(context, parenthesized));
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(edits)
                // `insert_before` / `insert_after` are given the operation's range, not the range
                // this offense reports, and that range is what orders them against each other.
                .corrections_anchored_at(operation.byte_range()),
        );
    }
}

fn insert(at: usize, text: impl Into<String>) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.into(),
        safe: true,
    }
}

/// `indentation_and_offset`: the block is written where the expression stood, one level deeper for
/// its body. Parentheses that are about to go take a column with them.
fn indentation_and_offset(
    context: &RuleContext<'_>,
    node: Node<'_>,
    width: usize,
    parenthesized: Option<Node<'_>>,
) -> (String, String) {
    let column = context.source.line_column(node.start_byte()).1 - 1;
    let column = match parenthesized.is_some() {
        true => column.saturating_sub(1),
        false => column,
    };
    (" ".repeat(column + width), " ".repeat(column))
}

/// `heredoc_end`: the end of the terminator of the last heredoc the operation opened as an
/// argument.
fn heredoc_end(context: &RuleContext<'_>, operation: Node<'_>) -> Option<usize> {
    if operation.kind() != "call" {
        return None;
    }
    let beginning = send_node::arguments(operation)
        .iter()
        .rev()
        .map(send_node::Argument::first)
        .find(|argument| argument.kind() == "heredoc_beginning")?;
    let body = send_node::heredoc_body(beginning, context)?;
    super::nodes::children(body)
        .into_iter()
        .find(|child| child.kind() == "heredoc_end")
        .map(|terminator| terminator.end_byte())
}
