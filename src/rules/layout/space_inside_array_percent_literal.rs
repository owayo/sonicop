//! `Layout/SpaceInsideArrayPercentLiteral`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MESSAGE: &str = "Use only a single space inside array percent literal.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for node in context.nodes_of_any(&["string_array", "symbol_array"]) {
        let count = node.child_count();
        let (Some(open), Some(close)) = (
            node.child(0),
            node.child(u32::try_from(count).unwrap_or(0).saturating_sub(1)),
        ) else {
            continue;
        };
        // `percent_literal?`, and the `%i %I %w %W` the cop asks `process` for. A `[1, 2]` array
        // has no percent opener, and the four are the only percent literals `on_array` can see.
        if !matches!(open.kind(), "%w(" | "%i(") || open.end_byte() > close.start_byte() {
            continue;
        }
        let contents = open.end_byte()..close.start_byte();
        for range in unnecessary_spaces(&text[contents.clone()]) {
            let range = (contents.start + range.start)..(contents.start + range.end);
            offenses.push(context.offense(MESSAGE, range.clone()).corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: " ".to_owned(),
                safe: true,
            }));
        }
    }
}

/// The runs `(?:[\S&&[^\\]](?:\\ )*)( {2,})(?=\S)` captures: two or more spaces between two words,
/// where the word before them may end in escaped spaces of its own.
fn unnecessary_spaces(contents: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = contents.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b' ' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        // `( {2,})(?=\S)`: a single space separates words already, and a run that ends the literal
        // or a line is not between two words.
        if index - start < 2
            || bytes
                .get(index)
                .is_none_or(|byte| byte.is_ascii_whitespace())
        {
            continue;
        }
        if word_ends_before(bytes, start) {
            found.push(start..index);
        }
    }
    found
}

/// `[\S&&[^\\]](?:\\ )*` read right to left: the escaped spaces that belong to the word before the
/// run, and then the word's own last character.
fn word_ends_before(bytes: &[u8], start: usize) -> bool {
    let mut position = start;
    while position >= 2 && bytes[position - 1] == b' ' && bytes[position - 2] == b'\\' {
        position -= 2;
    }
    position > 0 && !bytes[position - 1].is_ascii_whitespace() && bytes[position - 1] != b'\\'
}
