//! `RangeHelp` spans the cops around conditionals share.

/// `range_with_surrounding_space(range, side: :left, newlines:)`.
///
/// `final_pos` walks over spaces and tabs first and only then over newlines, so a run of blanks
/// that precedes the newline is not reached again once the newline has been stepped over.
pub(super) fn extended_left(text: &str, mut start: usize, newlines: bool) -> usize {
    let bytes = text.as_bytes();
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    if newlines {
        while start > 0 && bytes[start - 1] == b'\n' {
            start -= 1;
        }
    }
    start
}

/// `range_with_surrounding_space(range, side: :right, newlines:)`.
pub(super) fn extended_right(text: &str, mut end: usize, newlines: bool) -> usize {
    let bytes = text.as_bytes();
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    if newlines {
        while end < bytes.len() && bytes[end] == b'\n' {
            end += 1;
        }
    }
    end
}
