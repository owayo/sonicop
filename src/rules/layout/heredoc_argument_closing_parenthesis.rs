//! `Layout/HeredocArgumentClosingParenthesis`: where the closing parenthesis of a call written
//! with a heredoc argument goes.

use std::ops::Range;

use tree_sitter::Node;

use super::support::end_keyword;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments, heredoc_body, is_plain_send};
use crate::rules::send_node::named_children_iter;

const MSG: &str = "Put the closing parenthesis for a method call with a HEREDOC parameter on the \
                   same line as the HEREDOC opening.";

/// The argument a heredoc was found in, as upstream's `extract_heredoc_argument` hands it out.
///
/// Upstream gets one node back. A brace-less hash is several nodes here -- the grammar never built
/// the `hash` its pairs would hang off -- so the span they cover stands in for the node upstream
/// would have measured, and any one part is enough to walk up from.
struct HeredocArgument<'tree> {
    range: Range<usize>,
    node: Node<'tree>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(argument) = extract_heredoc_argument(node, context) else {
            continue;
        };
        let Some(outermost) = outermost_send_on_same_line(&argument, context) else {
            continue;
        };
        if end_keyword_before_closing_parenthesis(node)
            || subsequent_closing_parentheses_in_same_line(outermost, context)
            || exist_argument_between_heredoc_end_and_closing_parentheses(node, context)
        {
            continue;
        }
        let Some(parentheses) = parentheses(outermost, context) else {
            continue;
        };
        let Some(last) = call_arguments(outermost).last().map(|last| last.range()) else {
            continue;
        };
        offenses.push(
            context
                .offense(MSG, (parentheses.end - 1)..parentheses.end)
                // Both insertions are `insert_after(node.last_argument, ...)`, which is what orders
                // the `)` before the `,` when a trailing comma is moved along with it.
                .corrections_anchored_at(last.clone())
                .corrected_by_all(autocorrect(context, parentheses, last)),
        );
    }
}

/// `extract_heredoc_argument`: the first argument a heredoc is written in.
fn extract_heredoc_argument<'tree>(
    call: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<HeredocArgument<'tree>> {
    call_arguments(call).into_iter().find_map(|argument| {
        argument
            .parts()
            .iter()
            .any(|part| extract_heredoc(*part, context).is_some())
            .then(|| HeredocArgument {
                range: argument.range(),
                node: argument.first(),
            })
    })
}

/// `extract_heredoc`: the heredoc a value is, opens a chain with, or holds under a hash key.
fn extract_heredoc<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() == "heredoc_beginning" {
        return Some(node);
    }
    if node.kind_str() == "call" {
        return single_line_send_with_heredoc_receiver(node, context);
    }
    // `node.values`: a hash holds its heredoc under one of its keys. A pair reached on its own is
    // the brace-less form, whose `hash` upstream builds and the grammar does not.
    let values: Vec<Node<'tree>> = match node.kind_str() {
        "hash" => {
            let _cursor = node.walk();
            named_children_iter(node, context)
                .filter(|child| child.kind_str() == "pair")
                .filter_map(|pair| pair.field("value"))
                .collect()
        }
        "pair" => node.field("value").into_iter().collect(),
        _ => return None,
    };
    values
        .into_iter()
        .find_map(|value| extract_heredoc(value, context))
}

/// `single_line_send_with_heredoc_receiver?`: `<<~SQL.strip`, where the call is over before the
/// heredoc it reads from is.
fn single_line_send_with_heredoc_receiver<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if !is_plain_send(node, context) {
        return None;
    }
    let receiver = node.field("receiver")?;
    if receiver.kind_str() != "heredoc_beginning" {
        return None;
    }
    let terminator = heredoc_terminator(receiver, context)?;
    (terminator.end > node.end_byte()).then_some(receiver)
}

/// `outermost_send_on_same_line`: the innermost call around the argument whose `)` was left behind
/// on a line of its own.
fn outermost_send_on_same_line<'tree>(
    argument: &HeredocArgument<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut previous = argument.node;
    let mut current = upstream_parent(previous)?;
    while !send_missing_closing_parens(current, previous, &argument.range, context) {
        previous = current;
        current = upstream_parent(current)?;
    }
    Some(current)
}

/// `send_missing_closing_parens?`.
fn send_missing_closing_parens(
    parent: Node<'_>,
    child: Node<'_>,
    argument: &Range<usize>,
    context: &RuleContext<'_>,
) -> bool {
    if parent.kind_str() != "call" || !is_argument_of(parent, child) {
        return false;
    }
    let Some(parentheses) = parentheses(parent, context) else {
        return false;
    };
    context.source.line_column(parentheses.end - 1).0 != context.source.line_column(argument.end).0
}

/// `node.parent`, with the argument list the grammar adds between a call and its arguments passed
/// over -- upstream hands an argument straight to the call it was written in.
fn upstream_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    match parent.kind_str() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}

/// `node.arguments.include?(child)`.
fn is_argument_of(call: Node<'_>, child: Node<'_>) -> bool {
    call_arguments(call)
        .iter()
        .any(|argument| argument.parts().iter().any(|part| part.id() == child.id()))
}

/// `end_keyword_before_closing_parenthesis?`: something the call is written inside closes with an
/// `end`, and moving the parenthesis would be reaching across it.
fn end_keyword_before_closing_parenthesis(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if end_keyword(ancestor).is_some() {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

/// `subsequent_closing_parentheses_in_same_line?`: the parenthesis already sits right after the one
/// its last argument closes with.
fn subsequent_closing_parentheses_in_same_line(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parentheses) = parentheses(node, context) else {
        return false;
    };
    let Some(last) = call_arguments(node).pop() else {
        return false;
    };
    // `loc?(:end)`: only a single node has a closing token of its own, and a brace-less hash --
    // which is the only argument written as several nodes -- has none.
    let [only] = last.parts() else {
        return false;
    };
    let Some(closing) = closing_token(*only) else {
        return false;
    };
    let outer = context.source.line_column(parentheses.end - 1);
    let inner = context.source.line_column(closing.start_byte());
    outer.0 == inner.0 && outer.1 == inner.1 + 1
}

/// `exist_argument_between_heredoc_end_and_closing_parentheses?`.
fn exist_argument_between_heredoc_end_and_closing_parentheses(
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let Some(parentheses) = parentheses(node, context) else {
        return true;
    };
    let closing = parentheses.end - 1;
    // `find_most_bottom_of_heredoc_end`: only an argument that is itself a heredoc has one.
    let Some(terminator) = call_arguments(node)
        .iter()
        .filter_map(|argument| match argument.parts() {
            [only] => heredoc_terminator(*only, context).map(|range| range.end),
            _ => None,
        })
        .max()
    else {
        return false;
    };
    terminator < closing
        && !context
            .source
            .slice(terminator..closing)
            .trim_matches(|character: char| character.is_whitespace() || character == '\0')
            .is_empty()
}

/// The rewrite `autocorrect` performs: the parenthesis moves up to the heredoc opening, and a
/// trailing comma written on either side of it moves with it.
fn autocorrect(
    context: &RuleContext<'_>,
    parentheses: Range<usize>,
    last: Range<usize>,
) -> Vec<Edit> {
    let end_pos = parentheses.end;
    let source = context.source.text().as_bytes();
    let mut edits = vec![
        // `remove_incorrect_closing_paren`.
        Edit {
            start: incorrect_parenthesis_removal_begin(context, end_pos),
            end: match source.get(end_pos) {
                Some(b',') => end_pos + 1,
                _ => end_pos,
            },
            replacement: String::new(),
            safe: true,
        },
        // `add_correct_closing_paren`.
        Edit {
            start: last.end,
            end: last.end,
            replacement: ")".to_owned(),
            safe: true,
        },
    ];
    if let Some(offset) = internal_trailing_comma_offset(context, last.end, end_pos - 1) {
        edits.push(Edit {
            start: last.end,
            end: last.end + offset,
            replacement: String::new(),
            safe: true,
        });
    }
    if let Some(offset) = external_trailing_comma_offset(context, end_pos) {
        // A comma written straight after the parenthesis has already gone with it.
        if source.get(end_pos) != Some(&b',') {
            edits.push(Edit {
                start: end_pos,
                end: end_pos + offset,
                replacement: String::new(),
                safe: true,
            });
        }
        edits.push(Edit {
            start: last.end,
            end: last.end,
            replacement: ",".to_owned(),
            safe: true,
        });
    }
    edits
}

/// `incorrect_parenthesis_removal_begin`: the line break before a parenthesis left alone on a line
/// goes with it.
fn incorrect_parenthesis_removal_begin(context: &RuleContext<'_>, end_pos: usize) -> usize {
    let line = context.source.line_column(end_pos - 1).0;
    let start = context.source.line_start(line);
    match safe_to_remove_line_containing_closing_paren(context.source.line(line)) && start > 0 {
        true => start - 1,
        false => end_pos - 1,
    }
}

/// `safe_to_remove_line_containing_closing_paren?`: `/^ *\) {0,20},{0,1} *$/`.
fn safe_to_remove_line_containing_closing_paren(line: &str) -> bool {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(rest) = line.trim_start_matches(' ').strip_prefix(')') else {
        return false;
    };
    if rest.bytes().all(|byte| byte == b' ') {
        return true;
    }
    let spaces = rest.len() - rest.trim_start_matches(' ').len();
    spaces <= 20
        && rest[spaces..]
            .strip_prefix(',')
            .is_some_and(|tail| tail.bytes().all(|byte| byte == b' '))
}

/// `internal_trailing_comma_offset_from_last_arg`: a comma written after the last argument but
/// still on its line.
fn internal_trailing_comma_offset(
    context: &RuleContext<'_>,
    last_end: usize,
    closing: usize,
) -> Option<usize> {
    if last_end >= closing {
        return None;
    }
    let text = context.source.slice(last_end..closing);
    let comma = text.find(',')?;
    let newline = text.find('\n')?;
    (comma <= newline).then_some(comma + 1)
}

/// `external_trailing_comma_offset_from_loc_end`: a comma written after the parenthesis.
fn external_trailing_comma_offset(context: &RuleContext<'_>, end_pos: usize) -> Option<usize> {
    let source = context.source.text().as_bytes();
    let mut offset = 0;
    while offset < 20 && source.get(end_pos + offset) == Some(&b' ') {
        offset += 1;
    }
    (source.get(end_pos + offset) == Some(&b',')).then_some(offset + 1)
}

/// The parentheses a call was written with, when it was written with any.
fn parentheses(call: Node<'_>, context: &RuleContext<'_>) -> Option<Range<usize>> {
    let list = call.field("arguments")?;
    let range = list.byte_range();
    let text = context.source.slice(range.clone());
    (text.starts_with('(') && text.ends_with(')')).then_some(range)
}

/// `node.arguments`, without the heredoc bodies the grammar parks in the argument list.
fn call_arguments<'tree>(call: Node<'tree>) -> Vec<Argument<'tree>> {
    arguments(call)
        .into_iter()
        .filter(|argument| !matches!(argument.parts(), [only] if only.kind_str() == "heredoc_body"))
        .collect()
}

/// `loc.heredoc_end`: where a heredoc's terminator is written.
fn heredoc_terminator(node: Node<'_>, context: &RuleContext<'_>) -> Option<Range<usize>> {
    if node.kind_str() != "heredoc_beginning" {
        return None;
    }
    let body = heredoc_body(node, context)?;
    let _cursor = body.walk();
    named_children_iter(body, context)
        .find(|child| child.kind_str() == "heredoc_end")
        .map(|child| child.byte_range())
}

/// `loc.end`: the token a node closes with, when it closes with one.
fn closing_token<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let last = last_child(node)?;
    if matches!(last.kind_str(), ")" | "]" | "}" | "end") {
        return Some(last);
    }
    // A call keeps its parenthesis inside the argument list and a loop its `end` inside the body,
    // where upstream's node holds both directly. A block is a node of its own upstream, and the
    // call written before it ends where it does.
    if !matches!(
        last.kind_str(),
        "argument_list" | "block" | "do_block" | "do" | "body_statement"
    ) {
        return None;
    }
    last_child(last).filter(|inner| matches!(inner.kind_str(), ")" | "]" | "}" | "end"))
}

fn last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)
}
