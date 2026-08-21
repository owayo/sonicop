//! Column widths as a terminal counts them.
//!
//! RuboCop measures alignment and draws its carets with `Unicode::DisplayWidth`, so a line holding
//! CJK text is wider than its character count and the two disagree about where a caret belongs.
//! Both the alignment cops and the `clang`-style formatters need the same answer, which is why this
//! lives outside either of them.
//!
//! The answer comes from [`crate::display_width_table`], generated from the gem itself. A hand
//! written table stood here before and drifted: it counted the combining marks U+3099 and U+309A
//! two columns each, because the blanket wide range `0x3041..=0x33FF` swallowed them, so a source
//! file holding decomposed Japanese -- which is what macOS hands over routinely -- drew ten carets
//! where RuboCop draws six. Reproducing an exception table by hand is what went wrong; the ranges
//! are now taken from the reference implementation rather than restated.

use crate::display_width_table::WIDTHS;

/// `Unicode::DisplayWidth.of`, called the way RuboCop calls it: no options, one character at a
/// time.
pub fn display_width(text: &str) -> i64 {
    // `of` clamps its own result at zero, which is observable because BACKSPACE is worth -1: the
    // gem answers 0 for `"ab\b\b\b\b"` rather than -2. Nothing is narrower than nothing.
    text.chars().map(character_width).sum::<i64>().max(0)
}

fn character_width(character: char) -> i64 {
    let code = character as u32;
    // The table holds only the ranges whose width is not 1, sorted by their first code point and
    // non-overlapping, so the range that could contain `code` is the last one starting at or below
    // it. Anything else is one column wide.
    let candidate = WIDTHS.partition_point(|(first, _, _)| *first <= code);
    candidate
        .checked_sub(1)
        .and_then(|index| WIDTHS.get(index))
        .filter(|(_, last, _)| code <= *last)
        .map_or(1, |(_, _, width)| i64::from(*width))
}

#[cfg(test)]
mod tests {
    use super::{WIDTHS, display_width};

    #[test]
    fn ascii_is_one_column_per_character() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("x = 1"), 5);
    }

    #[test]
    fn east_asian_wide_and_fullwidth_characters_take_two_columns() {
        assert_eq!(display_width("日本語"), 6);
        assert_eq!(display_width("ａ"), 2); // U+FF41 FULLWIDTH LATIN SMALL LETTER A
        assert_eq!(display_width("ｱ"), 1); // U+FF71 halfwidth katakana stays narrow
        assert_eq!(display_width("😀"), 2);
    }

    /// The regression this table was generated for. macOS hands over decomposed Japanese, and
    /// counting the voiced sound mark as a character of its own made every one of them two columns
    /// too wide: `x = "がが"` drew ten carets where RuboCop draws six.
    #[test]
    fn a_combining_mark_takes_no_columns_of_its_own() {
        assert_eq!(display_width("\u{304B}\u{3099}"), 2); // か + ◌゙ = が
        assert_eq!(display_width("\u{304C}"), 2); // the composed form agrees
        assert_eq!(display_width("e\u{0301}"), 1); // e + ◌́
        assert_eq!(display_width("\u{115F}"), 0); // HANGUL CHOSEONG FILLER
    }

    /// `of` never answers with less than zero, however many backspaces the text holds.
    #[test]
    fn backspace_takes_a_column_back_but_never_past_zero() {
        assert_eq!(display_width("a\u{0008}"), 0);
        assert_eq!(display_width("ab\u{0008}\u{0008}\u{0008}\u{0008}"), 0);
        assert_eq!(display_width("\u{0008}"), 0);
    }

    /// Not every character is one or two columns wide, which is why the table stores a width rather
    /// than a flag.
    #[test]
    fn the_two_em_dash_takes_three_columns() {
        assert_eq!(display_width("\u{2E3B}"), 3);
        assert_eq!(display_width("\u{2E3A}"), 2); // THREE-EM DASH's narrower sibling
    }

    /// The lookup is a binary search, so a regenerated table that is unsorted or overlapping would
    /// answer wrongly rather than fail to build.
    #[test]
    fn the_generated_table_is_sorted_and_disjoint() {
        for window in WIDTHS.windows(2) {
            let (first, last, width) = window[0];
            let (next_first, _, _) = window[1];
            assert!(first <= last, "range 0x{first:04X} ends before it starts");
            assert!(
                last < next_first,
                "range 0x{first:04X}..=0x{last:04X} overlaps 0x{next_first:04X}"
            );
            assert_ne!(width, 1, "the table only holds widths other than 1");
        }
    }
}
