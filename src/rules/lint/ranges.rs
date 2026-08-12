//! The two `RangeHelp` spans a removal is written with, shared by the cops that delete a line or a
//! word and have to take the surrounding whitespace with it.

use std::ops::Range;

use crate::rules::RuleContext;

/// `range_by_whole_lines(range, include_final_newline: true)`.
pub(super) fn whole_lines(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |offset| range.end + offset + 1);
    start..end
}

/// `range_with_surrounding_space(range, side: :right)`: the span plus the spaces and tabs that
/// follow it, which stop at the end of the line.
pub(super) fn with_space_on_right(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    let text = context.source.text().as_bytes();
    let mut end = range.end;
    while end < text.len() && matches!(text[end], b' ' | b'\t') {
        end += 1;
    }
    range.start..end
}
