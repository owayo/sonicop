//! `Layout/SpaceInsideStringInterpolation`.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let space_style = context.setting::<String>("EnforcedStyle").as_deref() == Some("space");
    for node in context.nodes_of("interpolation") {
        let count = node.child_count();
        let (Some(open), Some(close)) = (
            node.child(0),
            node.child(u32::try_from(count).unwrap_or(0).saturating_sub(1)),
        ) else {
            continue;
        };
        if open.kind_str() != "#{" || close.kind_str() != "}" {
            continue;
        }
        inspect_delimiters(
            context,
            offenses,
            open.byte_range(),
            close.byte_range(),
            space_style,
        );
    }

    // tree-sitter-ruby can take a `#` in an interpolating heredoc for the beginning of a Ruby
    // comment and swallow the real interpolations later on the same line. The scanner still
    // preserves that span as a `comment` child of the heredoc. Recover only active `#{...}` pairs
    // inside that faux comment; ordinary source comments never have a `heredoc_body` parent.
    for comment in context.nodes_of("comment").filter(|comment| {
        comment
            .parent_of(context)
            .is_some_and(|parent| parent.kind_str() == "heredoc_body")
    }) {
        for (open, close) in
            interpolations_in_faux_comment(context.source.text(), comment.byte_range())
        {
            inspect_delimiters(context, offenses, open, close, space_style);
        }
    }
}

fn inspect_delimiters(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    open: Range<usize>,
    close: Range<usize>,
    space_style: bool,
) {
    let text = context.source.text();
    // `begin_node.multiline?`, then `empty_brackets?`: `#{ }` holds no token between its
    // delimiters, so there is no interior for a space to be inside of.
    if context.source.line_column(open.start).0 != context.source.line_column(close.start).0
        || text[open.end..close.start].trim().is_empty()
    {
        return;
    }
    let after = spaces_after(text, open.end);
    let before = spaces_before(text, close.start);

    // The ranges the offenses report, in the order the cop adds them, and the rewrites it makes
    // for all of them at once.
    let (reported, mut edits): (Vec<Range<usize>>, Vec<Edit>) = match space_style {
        true => (
            [
                after.is_empty().then(|| open.clone()),
                before.is_empty().then(|| close.clone()),
            ]
            .into_iter()
            .flatten()
            .collect(),
            [
                after.is_empty().then(|| insert_space(open.end)),
                before.is_empty().then(|| insert_space(close.start)),
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

/// Interpolations hidden in a heredoc child that the grammar mislabeled as a comment.
fn interpolations_in_faux_comment(
    text: &str,
    range: Range<usize>,
) -> Vec<(Range<usize>, Range<usize>)> {
    let bytes = text.as_bytes();
    let mut pairs = Vec::new();
    let mut offset = range.start;
    while offset + 1 < range.end {
        if bytes[offset] == b'#'
            && bytes[offset + 1] == b'{'
            && !escaped_at(bytes, offset, range.start)
            && let Some(close) = matching_brace(bytes, offset + 2, range.end)
        {
            pairs.push((offset..offset + 2, close..close + 1));
            offset = close + 1;
        } else {
            offset += 1;
        }
    }
    pairs
}

fn matching_brace(bytes: &[u8], mut offset: usize, end: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut quote = None;
    while offset < end {
        let byte = bytes[offset];
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                offset = (offset + 2).min(end);
                continue;
            }
            if byte == delimiter {
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(offset);
                    }
                }
                b'\\' => {
                    offset = (offset + 2).min(end);
                    continue;
                }
                _ => {}
            }
        }
        offset += 1;
    }
    None
}

fn escaped_at(bytes: &[u8], offset: usize, lower_bound: usize) -> bool {
    let mut slashes = 0usize;
    let mut before = offset;
    while before > lower_bound && bytes[before - 1] == b'\\' {
        slashes += 1;
        before -= 1;
    }
    slashes % 2 == 1
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
