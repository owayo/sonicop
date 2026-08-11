//! Horizontal whitespace scanning shared by the spacing cops.

use std::ops::Range;

/// The run of spaces and tabs ending at `offset`.
pub(super) fn whitespace_before(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}

/// The run of spaces and tabs starting at `offset`.
pub(super) fn whitespace_after(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut end = offset;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    offset..end
}
