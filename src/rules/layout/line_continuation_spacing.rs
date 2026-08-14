//! `Layout/LineContinuationSpacing`: how much blank stands in front of a line-continuing backslash.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    if !text.contains('\\') {
        return;
    }
    let space_style = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style != "no_space");
    let ignored = ignored_ranges(context);
    // `last_line`: the line the last token sits on, so trailing text no token reaches is left out.
    let last_line = context
        .nodes()
        .map(|node| node.start_position().row + 1)
        .max()
        .unwrap_or(1);
    for line in 1..=last_line.min(context.source.line_count()) {
        let Some(range) = offensive_spacing(context, line, space_style) else {
            continue;
        };
        // `ignore_range?`: a backslash inside a literal or a comment is part of its text.
        if ignored.iter().any(|outer| contains(outer, &range)) {
            continue;
        }
        let message = if space_style {
            "Use one space in front of backslash."
        } else {
            "Use zero spaces in front of backslash."
        };
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: if space_style { " \\" } else { "\\" }.to_owned(),
            safe: true,
        }));
    }
}

/// `find_offensive_spacing`, as the span the correction rewrites: the blanks and the backslash.
fn offensive_spacing(
    context: &RuleContext<'_>,
    line: usize,
    space_style: bool,
) -> Option<Range<usize>> {
    let span = context.source.line_range(line);
    let text = context.source.text();
    let content = text[span.clone()].trim_end_matches('\n');
    if !content.ends_with('\\') {
        return None;
    }
    let backslash = span.start + content.len() - 1;
    let blanks = content[..content.len() - 1]
        .bytes()
        .rev()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    // `no_space` wants none, `space` wants exactly one.
    let offends = if space_style { blanks != 1 } else { blanks > 0 };
    offends.then(|| backslash - blanks..backslash + 1)
}

/// `ignored_literal_ranges` together with the comments.
fn ignored_ranges(context: &RuleContext<'_>) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = context
        .nodes_of_any(&["string", "string_array", "symbol_array", "heredoc_body"])
        .map(|node| node.byte_range())
        .collect();
    // `ignored_parent?`: upstream ignores the `str` children of a regexp or a backtick command,
    // which together cover everything the delimiters enclose. The grammar splits that text into
    // `string_content` and `escape_sequence` pieces -- a continuation is one of the latter -- so
    // the whole literal stands in for them.
    ranges.extend(
        context
            .nodes_of_any(&["regex", "subshell"])
            .map(|node| node.byte_range()),
    );
    ranges.extend(context.comment_ranges().iter().cloned());
    ranges
}

/// `Parser::Source::Range#contains?`: inside, and not the very same span.
fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    let start = (inner.start.cmp(&outer.start)) as i8;
    let end = (outer.end.cmp(&inner.end)) as i8;
    start + end >= 1
}
