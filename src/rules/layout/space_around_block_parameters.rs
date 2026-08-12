//! `Layout/SpaceAroundBlockParameters`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::{body_statements, final_pos};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyleInsidePipes")
        .unwrap_or_else(|| "no_space".to_owned());
    let text = context.source.text();

    for node in context.nodes_of_any(&["block", "do_block"]) {
        let Some(parameters) = node
            .child_by_field_name("parameters")
            .filter(|parameters| parameters.kind() == "block_parameters")
        else {
            continue;
        };
        let (Some(open), Some(close)) = pipes(parameters) else {
            continue;
        };
        let arguments = block_arguments(parameters);
        let (Some(first), Some(last)) = (arguments.first(), arguments.last()) else {
            continue;
        };

        if style == "no_space" {
            no_space(
                context,
                open.end_byte()..first.start_byte(),
                "Space before first",
                offenses,
            );
            no_space(
                context,
                last_end_inside_pipes(text, parameters, *last)..close.start_byte(),
                "Space after last",
                offenses,
            );
        } else if style == "space" {
            // `check_opening_pipe_space` reports the first parameter itself when the space is
            // missing, and the extra blank when there is more than one.
            if open.end_byte() == first.start_byte() {
                offenses.push(
                    context
                        .offense(
                            "Space before first block parameter missing.",
                            first.byte_range(),
                        )
                        .corrected_by(insert(first.start_byte())),
                );
            }
            no_space(
                context,
                open.end_byte()..first.start_byte().saturating_sub(1),
                "Extra space before first",
                offenses,
            );
            let last_end = last_end_inside_pipes(text, parameters, *last);
            if last_end == close.start_byte() {
                offenses.push(
                    context
                        .offense(
                            "Space after last block parameter missing.",
                            last.byte_range(),
                        )
                        .corrected_by(insert(last.end_byte())),
                );
            }
            no_space(
                context,
                (last_end + 1)..close.start_byte(),
                "Extra space after last",
                offenses,
            );
        }

        if let Some(body) = body_start(node) {
            if close.end_byte() == body {
                offenses.push(
                    context
                        .offense("Space after closing `|` missing.", close.byte_range())
                        .corrected_by(insert(close.end_byte())),
                );
            }
        }

        for argument in each_argument(&arguments) {
            let start = argument.start_byte();
            no_space(
                context,
                final_pos(text, start, false, true, false)..start.saturating_sub(1),
                "Extra space before",
                offenses,
            );
        }
    }
}

/// `check_no_space`: an empty span is fine, and one holding a line break is somebody else's
/// business.
fn no_space(
    context: &RuleContext<'_>,
    range: Range<usize>,
    message: &str,
    offenses: &mut Vec<Offense>,
) {
    if range.start >= range.end {
        return;
    }
    if context.source.text()[range.clone()].contains('\n') {
        return;
    }
    offenses.push(
        context
            .offense(
                format!("{message} block parameter detected."),
                range.clone(),
            )
            .corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: String::new(),
                safe: true,
            }),
    );
}

fn insert(offset: usize) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement: " ".to_owned(),
        safe: true,
    }
}

fn pipes<'tree>(parameters: Node<'tree>) -> (Option<Node<'tree>>, Option<Node<'tree>>) {
    let mut cursor = parameters.walk();
    let bars: Vec<Node<'tree>> = parameters
        .children(&mut cursor)
        .filter(|child| child.kind() == "|")
        .collect();
    match bars.len() {
        0 | 1 => (None, None),
        _ => (bars.first().copied(), bars.last().copied()),
    }
}

fn block_arguments<'tree>(parameters: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
        .collect()
}

/// `check_each_arg`, which descends into a destructured parameter as well as visiting it.
fn each_argument<'tree>(arguments: &[Node<'tree>]) -> Vec<Node<'tree>> {
    let mut flattened = Vec::new();
    for argument in arguments {
        if argument.kind() == "destructured_parameter" {
            flattened.extend(each_argument(&block_arguments(*argument)));
        }
        flattened.push(*argument);
    }
    flattened
}

/// `last_end_pos_inside_pipes`: a trailing comma belongs to the last parameter as far as the
/// closing pipe is concerned.
fn last_end_inside_pipes(text: &str, parameters: Node<'_>, last: Node<'_>) -> usize {
    let position = last.end_byte();
    match text[position..parameters.end_byte()].find(',') {
        Some(index) => position + index + 1,
        None => position,
    }
}

/// `block.body.source_range.begin_pos`: where the first statement of the block starts.
fn body_start(node: Node<'_>) -> Option<usize> {
    let body = node.child_by_field_name("body")?;
    body_statements(body)
        .first()
        .map(tree_sitter::Node::start_byte)
}
