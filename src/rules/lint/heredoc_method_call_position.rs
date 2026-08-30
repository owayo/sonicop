use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, heredoc_body};
use crate::rules::send_node::named_children_of;

const MSG: &str =
    "Put a method call with a HEREDOC receiver on the same line as the HEREDOC opening.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(heredoc) = heredoc_receiver(node) else {
            continue;
        };
        let Some(terminator) = heredoc_end(heredoc, context) else {
            continue;
        };
        // `correctly_positioned?`: the call was written before the body began.
        if terminator > node.end_byte() {
            continue;
        }
        let text = context.source.text();
        if terminator + 2 > text.len() {
            continue;
        }
        let range = terminator + 1..terminator + 2;
        let offense = context.offense(MSG, range);
        let Some(call_range) = repositionable_call_range(node, terminator, context) else {
            offenses.push(offense);
            continue;
        };
        let anchor = whole_lines(node.start_byte(), context);
        offenses.push(
            offense
                .corrections_anchored_at(anchor.clone())
                .corrected_by_all([
                    Edit {
                        start: call_range.start,
                        end: call_range.end,
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: anchor.end,
                        end: anchor.end,
                        replacement: context.source.slice(call_range).trim().to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `heredoc_node_descendent_receiver`: the heredoc at the head of the chain of calls.
fn heredoc_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node;
    while current.kind_str() == "call" {
        let receiver = current.field("receiver")?;
        if receiver.kind_str() == "heredoc_beginning" {
            return Some(receiver);
        }
        current = receiver;
    }
    None
}

/// `heredoc.location.heredoc_end.end_pos`: where the terminator line ends.
fn heredoc_end(heredoc: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    let body = heredoc_body(heredoc, context)?;
    named_children_of(body, context)
        .into_iter()
        .find(|child| child.kind_str() == "heredoc_end")
        .map(|end| end.end_byte())
}

/// `call_range_to_safely_reposition`: the span that can be lifted onto the opener's line, or
/// `None` when moving it would take something else along.
fn repositionable_call_range(
    node: Node<'_>,
    terminator: usize,
    context: &RuleContext<'_>,
) -> Option<Range<usize>> {
    if calls_on_multiple_lines(node, context) {
        return None;
    }
    let call_range = terminator..node.end_byte();
    let line_range = whole_lines(node.end_byte(), context);
    let call_source = context.source.slice(call_range.clone()).trim();
    let line_source = context.source.slice(line_range).trim();
    if call_source == line_source {
        return Some(call_range);
    }
    // `trailing_comma?`: the comma belongs to the list rather than to the call, and moves with it.
    (format!("{call_source},") == line_source).then(|| terminator..node.end_byte() + 1)
}

/// `calls_on_multiple_lines?`: the chain, or an argument list in it, spans more than the last line.
fn calls_on_multiple_lines(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let last_line = context.source.line_column(node.end_byte()).0;
    let mut current = node;
    while current.kind_str() == "call" {
        if context.source.line_column(current.end_byte()).0 != last_line {
            return true;
        }
        let spans_lines = arguments(current)
            .first()
            .zip(arguments(current).last())
            .is_some_and(|(first, last)| {
                context.source.line_column(first.range().start).0
                    != context.source.line_column(last.range().end).0
            });
        if spans_lines {
            return true;
        }
        let Some(receiver) = current.field("receiver") else {
            return false;
        };
        current = receiver;
    }
    false
}

/// `range_by_whole_lines`: the line the offset lies on, without its line break.
fn whole_lines(position: usize, context: &RuleContext<'_>) -> Range<usize> {
    let text = context.source.text();
    let start = text[..position].rfind('\n').map_or(0, |offset| offset + 1);
    let end = text[position..]
        .find('\n')
        .map_or(text.len(), |offset| position + offset);
    start..end
}
