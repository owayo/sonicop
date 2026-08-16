//! `RangeHelp` spans the cops around conditionals share.

/// `range_with_surrounding_space(range, side: :left, newlines:)`.
///
/// `final_pos` walks over spaces and tabs first and only then over newlines, so a run of blanks
/// that precedes the newline is not reached again once the newline has been stepped over.
pub(super) fn extended_left(text: &str, start: usize, newlines: bool) -> usize {
    crate::rules::support::final_pos(text, start, false, false, newlines, false)
}

/// `range_with_surrounding_space(range, side: :right, newlines:)`.
pub(super) fn extended_right(text: &str, end: usize, newlines: bool) -> usize {
    crate::rules::support::final_pos(text, end, true, false, newlines, false)
}
