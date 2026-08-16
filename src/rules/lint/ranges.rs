//! The two `RangeHelp` spans a removal is written with, shared by the cops that delete a line or a
//! word and have to take the surrounding whitespace with it.

use std::ops::Range;

use crate::rules::RuleContext;

/// `range_by_whole_lines(range, include_final_newline: true)`.
pub(super) fn whole_lines(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    crate::rules::support::whole_lines(range, context)
}

/// `range_with_surrounding_space(range, side: :right)`: the span plus the spaces and tabs that
/// follow it, which stop at the end of the line.
pub(super) fn with_space_on_right(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    crate::rules::support::range_with_surrounding_space(
        range,
        context.source.text(),
        crate::rules::support::Side::Right,
        false,
        false,
        false,
    )
}
