//! `Layout/SpaceInsideStringInterpolation`.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let space_style = context.setting::<String>("EnforcedStyle").as_deref() == Some("space");
    let text = context.source.text();
    for node in context.nodes_of("interpolation") {
        let count = node.child_count();
        let (Some(open), Some(close)) = (
            node.child(0),
            node.child(u32::try_from(count).unwrap_or(0).saturating_sub(1)),
        ) else {
            continue;
        };
        if open.kind() != "#{" || close.kind() != "}" {
            continue;
        }
        // `begin_node.multiline?`, then `empty_brackets?`: `#{ }` holds no token between its
        // delimiters, so there is no interior for a space to be inside of.
        if context.source.line_column(open.start_byte()).0
            != context.source.line_column(close.start_byte()).0
            || text[open.end_byte()..close.start_byte()].trim().is_empty()
        {
            continue;
        }
        let after = spaces_after(text, open.end_byte());
        let before = spaces_before(text, close.start_byte());

        // The ranges the offenses report, in the order the cop adds them, and the rewrites it makes
        // for all of them at once.
        let (reported, mut edits): (Vec<Range<usize>>, Vec<Edit>) = match space_style {
            true => (
                [
                    after.is_empty().then(|| open.byte_range()),
                    before.is_empty().then(|| close.byte_range()),
                ]
                .into_iter()
                .flatten()
                .collect(),
                [
                    after.is_empty().then(|| insert_space(open.end_byte())),
                    before.is_empty().then(|| insert_space(close.start_byte())),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
            false => (
                [after.clone(), before.clone()]
                    .into_iter()
                    .filter(|range| !range.is_empty())
                    .collect(),
                [after, before]
                    .into_iter()
                    .filter(|range| !range.is_empty())
                    .map(remove_range)
                    .collect(),
            ),
        };
        let message = match space_style {
            true => "Use space inside string interpolation.",
            false => "Do not use space inside string interpolation.",
        };
        // Only the first offense of a node carries the correction: the cop rewrites both sides at
        // once and then ignores the node, which leaves any later offense with an empty corrector.
        for range in reported {
            let offense = context.offense(message, range);
            offenses.push(match edits.is_empty() {
                true => offense,
                false => offense.corrected_by_all(std::mem::take(&mut edits)),
            });
        }
    }
}

fn insert_space(offset: usize) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement: " ".to_owned(),
        safe: true,
    }
}

fn remove_range(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// `side_space_range(side: :right)`: the blanks a token is followed by.
fn spaces_after(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = offset;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    offset..end
}

/// `side_space_range(side: :left)`: the blanks a token is preceded by.
fn spaces_before(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}
