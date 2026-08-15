//! `FirstElementLineBreak` and `MultilineElementLineBreaks`: the two mixins the eight cops that ask
//! where the elements of a multi-line list begin are built on.
//!
//! The mixins are written against nodes, and the cops here work in spans instead. Two of the lists
//! have to be measured from the source rather than read off the tree: a percent literal's elements,
//! which the grammar mis-splits as soon as a backslash is written in one, and a heredoc's body, which
//! the grammar hangs inside the list the opener was written in while upstream's parser keeps it out.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children};

/// `method_uses_parens?`: the text before the first element closes with the parenthesis that opened
/// the list.
static OPENS_WITH_PARENTHESIS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\(\s*$").expect("the pattern compiles"));

/// The array literals upstream's parser builds an `array` node for. A percent literal is one, so is
/// the bracketless list on the right of `x = 1, 2`, and so is a `rescue`'s list of exceptions.
pub(super) const ARRAYS: &[&str] = &[
    "array",
    "string_array",
    "symbol_array",
    "right_assignment_list",
    "exceptions",
];

/// `check_children_line_break`: the first element shares the list's own line while something else in
/// the list does not.
pub(super) fn children_line_break(
    context: &RuleContext<'_>,
    start: Node<'_>,
    children: &[Range<usize>],
    ignore_last: bool,
    message: &'static str,
) -> Option<Offense> {
    let line = line_of(start.start_byte(), context);
    let min = children
        .iter()
        .min_by_key(|child| line_of(child.start, context))?
        .clone();
    if line != line_of(min.start, context) {
        return None;
    }
    // `last_line`: with `AllowMultilineFinalElement` the list only has to *start* on one line.
    let max_line = children
        .iter()
        .map(|child| match ignore_last {
            true => line_of(child.start, context),
            false => line_of(child.end, context),
        })
        .max()?;
    (line != max_line).then(|| insert_break(context, min, message))
}

/// `check_method_line_break`: the same, for a list a parenthesis opened.
pub(super) fn method_line_break(
    context: &RuleContext<'_>,
    node: Node<'_>,
    children: &[Range<usize>],
    ignore_last: bool,
    message: &'static str,
) -> Option<Offense> {
    let first = children.first()?;
    if !uses_parenthesis(context, node, first.start) {
        return None;
    }
    children_line_break(context, node, children, ignore_last, message)
}

/// `check_line_breaks`: every element after the first that shares a line with the one before it.
pub(super) fn line_breaks(
    context: &RuleContext<'_>,
    children: &[Range<usize>],
    ignore_last: bool,
    message: &'static str,
) -> Vec<Offense> {
    if all_on_same_line(context, children, ignore_last) {
        return Vec::new();
    }
    let mut last_seen: Option<usize> = None;
    let mut offenses = Vec::new();
    for child in children {
        match last_seen {
            Some(seen) if seen >= line_of(child.start, context) => {
                offenses.push(insert_break(context, child.clone(), message));
            }
            _ => last_seen = Some(line_of(child.end, context)),
        }
    }
    offenses
}

/// `all_on_same_line?`. With `ignore_last` the two ends are compared by the line they *start* on,
/// which is what `same_line?` reads.
fn all_on_same_line(
    context: &RuleContext<'_>,
    children: &[Range<usize>],
    ignore_last: bool,
) -> bool {
    let (Some(first), Some(last)) = (children.first(), children.last()) else {
        return true;
    };
    match ignore_last {
        true => line_of(first.start, context) == line_of(last.start, context),
        false => line_of(first.start, context) == line_of(last.end, context),
    }
}

/// `method_uses_parens?`.
fn uses_parenthesis(context: &RuleContext<'_>, node: Node<'_>, limit: usize) -> bool {
    let line = context.source.line(line_of(node.start_byte(), context));
    let column = context.source.line_column(limit).1 - 1;
    let prefix: String = line.chars().take(column).collect();
    OPENS_WITH_PARENTHESIS.is_match(&prefix)
}

/// `EmptyLineCorrector.insert_before`: a line break goes in front of the element.
fn insert_break(context: &RuleContext<'_>, range: Range<usize>, message: &'static str) -> Offense {
    let start = range.start;
    context.offense(message, range).corrected_by(Edit {
        start,
        end: start,
        replacement: "\n".to_owned(),
        safe: true,
    })
}

pub(super) fn line_of(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}

/// `node.children` for a literal: the spans of its elements.
pub(super) fn elements(node: Node<'_>, context: &RuleContext<'_>) -> Vec<Range<usize>> {
    if matches!(node.kind_str(), "string_array" | "symbol_array") {
        return percent_elements(node, context);
    }
    let children = listed(node);
    match ARRAYS.contains(&node.kind_str()) {
        // A run of `key: value` elements written inside an array is one `hash` child upstream.
        true => folded(&children),
        // A hash owns its pairs directly, and a parameter list has none.
        false => children.iter().map(|child| child.byte_range()).collect(),
    }
}

/// The spans the children stand for, with each run of `key: value` elements folded into the one
/// braceless `hash` upstream's parser builds out of it.
fn folded(children: &[Node<'_>]) -> Vec<Range<usize>> {
    let mut spans: Vec<Range<usize>> = Vec::with_capacity(children.len());
    let mut open = false;
    for child in children {
        match is_hash_element(*child) {
            true if open => {
                if let Some(last) = spans.last_mut() {
                    last.end = child.end_byte();
                }
            }
            true => {
                spans.push(child.byte_range());
                open = true;
            }
            false => {
                spans.push(child.byte_range());
                open = false;
            }
        }
    }
    spans
}

fn is_hash_element(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "pair" | "hash_splat_argument")
}

/// The children of a list upstream's parser also has: the comments the grammar keeps and the heredoc
/// body it hangs here are neither.
fn listed<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(node)
        .into_iter()
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect()
}

/// `node.arguments`, with a trailing braceless hash replaced by the pairs it was written as -- which
/// is what both argument cops do before they measure.
///
/// Only the *last* argument is broken up: `mail(to: 1, from: 2, &block)` hands the pairs to one
/// `hash` argument that is not last, and both cops then measure that hash whole.
pub(super) fn expanded_arguments(node: Node<'_>) -> Vec<Range<usize>> {
    let arguments = arguments(node);
    let mut spans = Vec::with_capacity(arguments.len());
    for (index, argument) in arguments.iter().enumerate() {
        let last = index + 1 == arguments.len();
        let braceless_hash = is_hash_element(argument.first());
        match last && braceless_hash {
            true => spans.extend(argument.parts().iter().map(|part| part.byte_range())),
            false => spans.push(argument.range()),
        }
    }
    spans
        .into_iter()
        .filter(|span| !is_heredoc_body(node, span))
        .collect()
}

/// Whether the span is the heredoc body the grammar hangs inside the argument list and upstream's
/// parser keeps out of it.
fn is_heredoc_body(node: Node<'_>, span: &Range<usize>) -> bool {
    node.field("arguments").is_some_and(|list| {
        named_children(list)
            .iter()
            .any(|child| child.kind_str() == "heredoc_body" && child.byte_range() == *span)
    })
}

/// The index arguments of `a[1, 2]`, which upstream reads as the arguments of a `:[]` send.
pub(super) fn indices(node: Node<'_>) -> Vec<Range<usize>> {
    let children = listed(node);
    children
        .get(1..)
        .unwrap_or_default()
        .iter()
        .map(|child| child.byte_range())
        .collect()
}

/// Whether the index is being written to, which makes it a `:[]=` send rather than a `:[]` one.
pub(super) fn is_assignment_target(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| match parent.kind_str() {
        "assignment" | "operator_assignment" => parent.field("left") == Some(node),
        "left_assignment_list" => true,
        _ => false,
    })
}

/// The elements of a `%w[]` or `%i[]`, measured from the source.
///
/// Ruby splits a percent literal on unescaped blanks, and the grammar does not: `%w[\a \b]` reaches
/// it as one element that begins where the literal's own text does. Interpolation is one unit, so a
/// blank written inside a `#{}` does not separate anything.
fn percent_elements(node: Node<'_>, context: &RuleContext<'_>) -> Vec<Range<usize>> {
    let text = context.source.text().as_bytes();
    let (Some(open), Some(close)) = (
        interior_start(node, context),
        node.end_byte().checked_sub(1),
    ) else {
        return Vec::new();
    };
    let mut elements = Vec::new();
    let mut current: Option<usize> = None;
    let mut offset = open;
    while offset < close {
        match text[offset] {
            b'\\' => {
                current = current.or(Some(offset));
                offset += 2;
            }
            b'#' if text.get(offset + 1) == Some(&b'{') => {
                current = current.or(Some(offset));
                offset = interpolation_end(text, offset + 2, close);
            }
            byte if byte.is_ascii_whitespace() => {
                if let Some(start) = current.take() {
                    elements.push(start..offset);
                }
                offset += 1;
            }
            _ => {
                current = current.or(Some(offset));
                offset += 1;
            }
        }
    }
    if let Some(start) = current {
        elements.push(start..close.min(offset));
    }
    elements
}

/// Where a percent literal's contents begin: after its `%`, its optional letter and its delimiter.
fn interior_start(node: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    let text = context.source.text().as_bytes();
    let start = node.start_byte();
    if text.get(start) != Some(&b'%') {
        return None;
    }
    match text.get(start + 1)?.is_ascii_alphabetic() {
        true => Some(start + 3),
        false => Some(start + 2),
    }
}

/// Where the `}` that closes an interpolation opened at `offset` leaves off.
fn interpolation_end(text: &[u8], offset: usize, limit: usize) -> usize {
    let mut depth = 1_usize;
    let mut offset = offset;
    while offset < limit {
        match text[offset] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return offset + 1;
                }
            }
            _ => {}
        }
        offset += 1;
    }
    limit
}
