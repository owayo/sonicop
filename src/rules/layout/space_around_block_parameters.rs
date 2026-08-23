//! `Layout/SpaceAroundBlockParameters`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::{body_statements, final_pos};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyleInsidePipes")
        .unwrap_or_else(|| "no_space".to_owned());
    let text = context.source.text();

    // A lambda literal is a `block` node upstream as well (`(block (lambda) (args ...) body)`), so
    // `on_block` sees `->( x, y) { }` and reports its parentheses under the same messages as a
    // block's pipes. The grammar gives the literal a node of its own, so it has to be walked here
    // too -- leaving it out was 8 missed offenses in the cop's own spec.
    for node in context.nodes_of_any(&["block", "do_block", "lambda"]) {
        let Some(parameters) = node.field("parameters").filter(|parameters| {
            matches!(
                parameters.kind_str(),
                "block_parameters" | "lambda_parameters"
            )
        }) else {
            continue;
        };
        let (Some(open), Some(close)) = delimiters(parameters) else {
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
                final_pos(text, start, false, false, true, false)..start.saturating_sub(1),
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
    // `add_offense` keeps one offense per range (`current_offense_locations.add?`). The pipe
    // checks and the per-argument check reach the same span for the first parameter, and upstream
    // reports only the one that got there first.
    if offenses
        .iter()
        .any(|offense| offense.start == range.start && offense.end == range.end)
    {
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

/// `pipes`: `[arguments.loc.begin, arguments.loc.end]`.
///
/// **A lambda's parameter list is delimited by parentheses, and upstream reads the same two
/// locations for it** -- `->( x, y)` is reported as "Space before first block parameter" exactly as
/// `{ | x, y| }` is. A list written without them (`->x { }`, `{ x }`) has neither location, which is
/// what `pipes?` stops the cop on.
fn delimiters<'tree>(parameters: Node<'tree>) -> (Option<Node<'tree>>, Option<Node<'tree>>) {
    let (opening, closing) = match parameters.kind_str() {
        "lambda_parameters" => ("(", ")"),
        _ => ("|", "|"),
    };
    let mut cursor = parameters.walk();
    let children: Vec<Node<'tree>> = parameters.children(&mut cursor).collect();
    let open = children
        .iter()
        .find(|child| child.kind_str() == opening)
        .copied();
    let close = children
        .iter()
        .rev()
        .find(|child| child.kind_str() == closing)
        .copied();
    match (open, close) {
        // One `|` is not a pair, and neither is a `(` the parser recovered without its `)`.
        (Some(open), Some(close)) if open.id() != close.id() => (Some(open), Some(close)),
        _ => (None, None),
    }
}

fn block_arguments<'tree>(parameters: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect()
}

/// `check_each_arg`, which descends into a destructured parameter as well as visiting it.
fn each_argument<'tree>(arguments: &[Node<'tree>]) -> Vec<Node<'tree>> {
    let mut flattened = Vec::new();
    for argument in arguments {
        if argument.kind_str() == "destructured_parameter" {
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
///
/// A lambda literal's `body` field is the brace block itself rather than its statements, so that
/// one takes a further step in. Upstream reads `arguments.parent.body`, and the parent of a
/// lambda's argument list is the same `block` node a brace block has.
fn body_start(node: Node<'_>) -> Option<usize> {
    let body = node.field("body")?;
    let body = match body.kind_str() {
        "block" | "do_block" => body.field("body")?,
        _ => body,
    };
    body_statements(body)
        .first()
        .map(tree_sitter::Node::start_byte)
}
