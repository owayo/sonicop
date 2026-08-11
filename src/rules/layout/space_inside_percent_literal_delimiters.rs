//! `Layout/SpaceInsidePercentLiteralDelimiters`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MESSAGE: &str = "Do not use spaces inside percent literal delimiters.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    // `on_array` covers `%i`, `%I`, `%w` and `%W`; `on_xstr` covers `%x`. A backtick command is an
    // `xstr` too, so the opening delimiter has to start with a percent sign for either to apply.
    let mut reported: HashSet<(usize, usize)> = HashSet::new();
    for node in context.nodes_of_any(&["string_array", "symbol_array", "subshell"]) {
        let Some((open, close)) = delimiters(node) else {
            continue;
        };
        if !text[open.clone()].starts_with('%') {
            continue;
        }
        let contents = open.end..close.start;
        blank_offense(context, contents.clone(), &mut reported, offenses);
        if node.start_position().row != node.end_position().row {
            continue;
        }
        for range in edge_spaces(text, contents) {
            push(context, range, &mut reported, offenses);
        }
    }
}

/// A literal whose body is nothing but whitespace, which is removed whole.
fn blank_offense(
    context: &RuleContext<'_>,
    contents: Range<usize>,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    let body = &context.source.text()[contents.clone()];
    if body.is_empty() || !body.trim().is_empty() {
        return;
    }
    push(context, contents, reported, offenses);
}

/// The leading and trailing runs of spaces `BEGIN_REGEX` and `END_REGEX` match. A trailing run is
/// cut short by a backslash, which escapes the space that follows it into part of the last word.
fn edge_spaces(text: &str, contents: Range<usize>) -> Vec<Range<usize>> {
    let body = &text[contents.clone()];
    let mut ranges = Vec::new();
    let leading = body.bytes().take_while(|byte| *byte == b' ').count();
    if leading > 0 {
        ranges.push(contents.start..contents.start + leading);
    }
    let trailing = body.bytes().rev().take_while(|byte| *byte == b' ').count();
    if trailing > 0 {
        // `scan` reports the earliest match, so the run starts at the first of the trailing spaces
        // that is not itself preceded by a backslash.
        let mut start = contents.end - trailing;
        while start < contents.end && start > contents.start && text.as_bytes()[start - 1] == b'\\'
        {
            start += 1;
        }
        if start < contents.end {
            ranges.push(start..contents.end);
        }
    }
    ranges
}

fn push(
    context: &RuleContext<'_>,
    range: Range<usize>,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    if !reported.insert((range.start, range.end)) {
        return;
    }
    offenses.push(context.offense(MESSAGE, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }));
}

/// The literal's opening and closing delimiter spans.
fn delimiters(node: Node<'_>) -> Option<(Range<usize>, Range<usize>)> {
    let first = node.child(0)?;
    let last = node.child(u32::try_from(node.child_count()).ok()?.saturating_sub(1))?;
    if first.end_byte() > last.start_byte() {
        return None;
    }
    Some((first.byte_range(), last.byte_range()))
}
