//! `Layout/EmptyLinesAroundArguments`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::{final_pos, grouped_arguments, is_send_like};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Empty line detected around arguments.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference", "assignment", "binary"]) {
        // `on_send` and `on_csend` are all this cop handles: **no `on_super`**, so a blank line
        // between `super`'s arguments is left alone upstream. The grammar writes `super(...)` as a
        // `call`, and removing that line is a change upstream never makes.
        if crate::rules::send_node::is_super_call(node) {
            continue;
        }
        let Some(send) = send_view(context, node) else {
            continue;
        };
        if send.first_line == send.last_line || send.arguments.is_empty() {
            continue;
        }
        // `receiver_and_method_call_on_different_lines?`
        if send
            .receiver_last_line
            .is_some_and(|line| Some(line) != send.selector_line)
        {
            continue;
        }
        for start in send.arguments.iter().copied().chain(send.close) {
            if let Some(range) = empty_range_before(context, start) {
                offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }));
            }
        }
    }
}

/// `empty_range_for_starting_point`: the blank line immediately before `start`, when the run of
/// whitespace reaching back from it spans more than one line break.
fn empty_range_before(context: &RuleContext<'_>, start: usize) -> Option<Range<usize>> {
    let begin = final_pos(context.source.text(), start, false, false, true, true);
    let first_line = context.source.line_column(begin).0;
    let last_line = context.source.line_column(start).0;
    (last_line.checked_sub(first_line)? > 1).then(|| context.source.line_range(last_line - 1))
}

/// What one `send` looks like to this cop. The grammar spells a call several ways -- an index read,
/// an index or attribute write, an operator -- that upstream's parser all files under `send`.
struct SendView {
    first_line: usize,
    last_line: usize,
    receiver_last_line: Option<usize>,
    selector_line: Option<usize>,
    /// Where each argument starts, grouped the way `SendNode#arguments` presents them.
    arguments: Vec<usize>,
    /// `node.loc.end`: the bracket or parenthesis the call closes with.
    close: Option<usize>,
}

fn send_view(context: &RuleContext<'_>, node: Node<'_>) -> Option<SendView> {
    let line = |offset: usize| context.source.line_column(offset).0;
    let starts = |ranges: Vec<Range<usize>>| ranges.into_iter().map(|range| range.start).collect();
    let view = |node: Node<'_>, receiver: Option<Node<'_>>, selector: Option<Node<'_>>| SendView {
        first_line: line(node.start_byte()),
        last_line: line(node.end_byte()),
        receiver_last_line: receiver.map(|receiver| line(receiver.end_byte())),
        selector_line: selector.map(|selector| line(selector.start_byte())),
        arguments: Vec::new(),
        close: None,
    };

    match node.kind_str() {
        "call" => {
            // An attribute write is one `:name=` send spanning the assignment, so the call on its
            // own is not what upstream inspects.
            if is_assignment_target(node, context) {
                return None;
            }
            let list = child_of_kind(node, "argument_list");
            Some(SendView {
                arguments: starts(
                    grouped_arguments(node)
                        .into_iter()
                        .map(|argument| argument.range)
                        .collect(),
                ),
                close: list.and_then(closing_delimiter),
                ..view(node, node.field("receiver"), selector(node))
            })
        }
        "element_reference" => {
            if is_assignment_target(node, context) {
                return None;
            }
            let bracket = child_of_kind(node, "[")?;
            Some(SendView {
                arguments: starts(
                    grouped_arguments(node)
                        .into_iter()
                        .map(|argument| argument.range)
                        .collect(),
                ),
                // `Map::Send` carries no `end` for a `:[]` send, so there is no closing token to
                // look at even though the brackets are right there.
                close: None,
                ..view(node, node.child(0), Some(bracket))
            })
        }
        // `a[0] = 1` is a single `:[]=` send and `a.b = 1` a single `:b=` send, each holding the
        // right-hand side as one more argument.
        "assignment" => {
            let left = node.field("left")?;
            let right = node.field("right")?;
            let (receiver, selector, mut arguments, close) = match left.kind_str() {
                "element_reference" => (
                    left.child(0),
                    child_of_kind(left, "["),
                    starts(
                        grouped_arguments(left)
                            .into_iter()
                            .map(|argument| argument.range)
                            .collect(),
                    ),
                    None,
                ),
                "call" => (left.field("receiver"), selector(left), Vec::new(), None),
                _ => return None,
            };
            arguments.push(right.start_byte());
            Some(SendView {
                arguments,
                close,
                ..view(node, receiver, selector)
            })
        }
        "binary" if is_send_like(context, node) => {
            let operator = node.field("operator")?;
            let right = node.field("right")?;
            Some(SendView {
                arguments: vec![right.start_byte()],
                ..view(node, node.field("left"), Some(operator))
            })
        }
        _ => None,
    }
}

fn selector<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("method")
        .filter(|method| !method.byte_range().is_empty())
}

fn is_assignment_target(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    context.parent(node).is_some_and(|parent| {
        parent.kind_str() == "assignment" && parent.field("left") == Some(node)
    })
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == kind)
}

/// The closing parenthesis of an argument list written with them.
fn closing_delimiter(list: Node<'_>) -> Option<usize> {
    let first = list.child(0)?;
    if first.kind_str() != "(" {
        return None;
    }
    let last = list.child(u32::try_from(list.child_count()).ok()?.checked_sub(1)?)?;
    (last.kind_str() == ")").then(|| last.start_byte())
}
