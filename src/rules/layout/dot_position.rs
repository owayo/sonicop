//! `Layout/DotPosition`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::heredoc_terminators;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let leading = context
        .setting::<String>("EnforcedStyle")
        .as_deref()
        .map(|style| style != "trailing")
        .unwrap_or(true);
    let heredocs = heredoc_terminators(context);
    for node in context.nodes_of("call") {
        let Some(dot) = node.child_by_field_name("operator") else {
            continue;
        };
        let dot_text = &context.source.text()[dot.byte_range()];
        // `node.dot? || node.safe_navigation?`: `Foo::bar` belongs to `Style/ColonMethodCall`.
        if !matches!(dot_text, "." | "&.") {
            continue;
        }
        let (Some(receiver), Some(selector)) =
            (node.child_by_field_name("receiver"), selector(node))
        else {
            continue;
        };
        let selector_line = context.source.line_column(selector.start).0;
        if selector_line == context.source.line_column(receiver.end_byte()).0 {
            continue;
        }
        let dot_line = context.source.line_column(dot.start_byte()).0;
        let receiver_line = receiver_end_line(context, &heredocs, receiver);
        // A blank or comment line between the two halves of the call would be lost by the
        // correction, so upstream leaves such a call alone.
        if selector_line > receiver_line.max(dot_line) + 1 {
            continue;
        }
        if leading == (dot_line == selector_line) {
            continue;
        }
        let position = match leading {
            true => "next line, together with the method name.",
            false => "previous line, together with the method call receiver.",
        };
        let (removed, anchor, offset) = match leading {
            true => (
                removed_range(context, dot, dot_line),
                selector.clone(),
                selector.start,
            ),
            false => (
                removed_range(context, dot, dot_line),
                receiver.byte_range(),
                receiver.end_byte(),
            ),
        };
        offenses.push(
            context
                .offense(
                    format!("Place the {dot_text} on the {position}"),
                    dot.byte_range(),
                )
                .corrected_by_all([
                    Edit {
                        start: removed.start,
                        end: removed.end,
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: offset,
                        end: offset,
                        replacement: dot_text.to_owned(),
                        safe: true,
                    },
                ])
                .corrections_anchored_at(anchor),
        );
    }
}

/// The text the correction takes out: the dot, or the whole line when the dot is all that is on it.
fn removed_range(context: &RuleContext<'_>, dot: Node<'_>, dot_line: usize) -> Range<usize> {
    if context.source.line(dot_line).trim() != "." {
        return dot.byte_range();
    }
    let line = context.source.line_range(dot_line);
    line.start..line.end
}

/// `selector_range`: the method name, or the opening parenthesis of a `foo.(1)` call, which has no
/// name to point at.
fn selector(node: Node<'_>) -> Option<Range<usize>> {
    if let Some(method) = node.child_by_field_name("method") {
        return Some(method.byte_range());
    }
    let arguments = node.child_by_field_name("arguments")?;
    let open = arguments.child(0).filter(|child| child.kind() == "(")?;
    Some(open.byte_range())
}

/// `receiver_end_line`: where the receiver really ends, which for one carrying a heredoc is the
/// terminator lines below rather than the line the opener was written on.
fn receiver_end_line(
    context: &RuleContext<'_>,
    heredocs: &[(usize, Range<usize>)],
    receiver: Node<'_>,
) -> usize {
    let terminator = last_heredoc(heredocs, receiver);
    match terminator {
        Some(terminator) => context.source.line_column(terminator.start).0,
        None => context.source.line_column(receiver.end_byte()).0,
    }
}

/// `last_heredoc_line`: the heredocs written as direct arguments of a call, or the receiver itself
/// when it is a heredoc.
fn last_heredoc(heredocs: &[(usize, Range<usize>)], receiver: Node<'_>) -> Option<Range<usize>> {
    let terminator_of = |opener: usize| {
        heredocs
            .iter()
            .find(|(start, _)| *start == opener)
            .map(|(_, terminator)| terminator.clone())
    };
    if receiver.kind() == "heredoc_beginning" {
        return terminator_of(receiver.start_byte());
    }
    if receiver.kind() != "call" {
        return None;
    }
    let arguments = receiver.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    arguments
        .named_children(&mut cursor)
        .filter(|argument| argument.kind() == "heredoc_beginning")
        .filter_map(|argument| terminator_of(argument.start_byte()))
        .max_by_key(|terminator| terminator.end)
}
