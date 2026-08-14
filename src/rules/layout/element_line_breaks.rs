//! `FirstElementLineBreak` and `MultilineElementLineBreaks`: the two mixins the eight line-break
//! cops are built from, plus the element lists they are handed.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `check_children_line_break`: the first element shares its line with the opening, and something
/// else does not fit on that line.
pub(super) fn check_children_line_break(
    context: &RuleContext<'_>,
    message: &'static str,
    start: usize,
    children: &[Range<usize>],
    ignore_last: bool,
    offenses: &mut Vec<Offense>,
) {
    let Some(first) = children
        .iter()
        .min_by_key(|child| line_of(context, child.start))
    else {
        return;
    };
    let line = line_of(context, start);
    if line != line_of(context, first.start) {
        return;
    }
    // `AllowMultilineFinalElement` measures each element by where it begins rather than where it
    // ends, so a last element spilling over does not count.
    let last_line = children
        .iter()
        .map(|child| {
            if ignore_last {
                line_of(context, child.start)
            } else {
                line_of(context, child.end)
            }
        })
        .max()
        .unwrap_or(line);
    if line == last_line {
        return;
    }
    offenses.push(break_before(context, message, first.clone()));
}

/// `check_line_breaks`: every element opens a line of its own.
pub(super) fn check_line_breaks(
    context: &RuleContext<'_>,
    message: &'static str,
    children: &[Range<usize>],
    ignore_last: bool,
    offenses: &mut Vec<Offense>,
) {
    let (Some(first), Some(last)) = (children.first(), children.last()) else {
        return;
    };
    let all_on_one_line = if ignore_last {
        line_of(context, first.start) == line_of(context, last.start)
    } else {
        line_of(context, first.start) == line_of(context, last.end)
    };
    if all_on_one_line {
        return;
    }
    let mut last_seen: i64 = -1;
    for child in children {
        if last_seen >= line_of(context, child.start) as i64 {
            offenses.push(break_before(context, message, child.clone()));
        } else {
            last_seen = line_of(context, child.end) as i64;
        }
    }
}

/// `EmptyLineCorrector.insert_before`.
fn break_before(
    context: &RuleContext<'_>,
    message: &'static str,
    element: Range<usize>,
) -> Offense {
    context
        .offense(message, element.clone())
        .corrected_by(Edit {
            start: element.start,
            end: element.start,
            replacement: "\n".to_owned(),
            safe: true,
        })
}

/// `method_uses_parens?`: what stands between the start of the call's line and the first element
/// ends with an opening parenthesis.
pub(super) fn method_uses_parens(
    context: &RuleContext<'_>,
    node_start: usize,
    first_element: usize,
) -> bool {
    let line = line_of(context, node_start);
    let text = context.source.line(line);
    let column = context.source.line_column(first_element).1 - 1;
    let prefix: String = text.chars().take(column).collect();
    prefix.trim_end().ends_with('(')
}

/// The elements of a literal, as upstream's `node.children`.
pub(super) fn literal_elements(node: Node<'_>) -> Vec<Range<usize>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .map(|child| child.byte_range())
        .collect()
}

/// A call's arguments, with a trailing brace-less hash spread back into its pairs -- which is what
/// `args.concat(args.pop.children)` does upstream.
pub(super) fn call_arguments(node: Node<'_>) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    for argument in arguments(node) {
        match argument.parts() {
            [single] => found.push(single.byte_range()),
            parts => found.extend(parts.iter().map(|part| part.byte_range())),
        }
    }
    found
}

fn line_of(context: &RuleContext<'_>, offset: usize) -> usize {
    context.source.line_column(offset).0
}
