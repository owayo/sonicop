//! A regexp literal, read as the pattern it holds rather than as the node it is.
//!
//! Upstream hands these cops a parsed regular expression through `regexp_parser`, and they ask it
//! about capture groups. Nothing here parses a regexp -- the questions the cops ask are answered by
//! a single scan that knows the three places a `(` is not the start of a group: inside a character
//! class, after a backslash, and in the `(?#...)` comment form.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::send_node::has_interpolation;

/// The capture groups a pattern declares, told apart the way `each_capture(named:)` does.
#[derive(Default)]
pub(super) struct Captures {
    pub(super) numbered: usize,
    pub(super) named: usize,
}

/// The text between a regexp literal's delimiters, and whether it was written with the `x` flag.
pub(super) fn pattern<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<(&'a str, bool)> {
    let opening = node.child(0)?;
    let closing = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
    if closing.start_byte() < opening.end_byte() {
        return None;
    }
    let extended = context.source.node_text(closing).contains('x');
    Some((
        context
            .source
            .slice(opening.end_byte()..closing.start_byte()),
        extended,
    ))
}

/// Whether the literal interpolates, which stops upstream from parsing it at all.
pub(super) fn interpolates(node: Node<'_>) -> bool {
    has_interpolation(node)
}

/// The capture groups of one pattern.
pub(super) fn captures(pattern: &str, extended: bool) -> Captures {
    let mut found = Captures::default();
    let bytes = pattern.as_bytes();
    let mut index = 0;
    let mut in_class = false;
    // The offset the current character class started at, so that a `]` written first is literal.
    let mut class_start = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'[' if !in_class => {
                in_class = true;
                class_start = index;
                index += 1;
            }
            b']' if in_class => {
                // `[]]` and `[^]]` open with a literal `]`.
                let first = class_start
                    + if bytes.get(class_start + 1) == Some(&b'^') {
                        2
                    } else {
                        1
                    };
                if index > first {
                    in_class = false;
                }
                index += 1;
            }
            b'#' if extended && !in_class => {
                index += pattern[index..]
                    .find('\n')
                    .map_or(bytes.len() - index, |offset| offset);
            }
            b'(' if !in_class => {
                index += group(&pattern[index..], &mut found);
            }
            _ => index += 1,
        }
    }
    found
}

/// Reads one `(`, counting the group it opens, and answers how much of the pattern it spanned.
fn group(rest: &str, found: &mut Captures) -> usize {
    let bytes = rest.as_bytes();
    if bytes.get(1) != Some(&b'?') {
        found.numbered += 1;
        return 1;
    }
    match bytes.get(2) {
        // `(?#...)` is a comment, which runs to the first unescaped `)`.
        Some(b'#') => {
            let mut index = 3;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index += 2,
                    b')' => return index + 1,
                    _ => index += 1,
                }
            }
            bytes.len()
        }
        // `(?<name>` captures, while `(?<=` and `(?<!` look behind.
        Some(b'<') if !matches!(bytes.get(3), Some(b'=' | b'!')) => {
            found.named += 1;
            3
        }
        Some(b'\'') => {
            found.named += 1;
            3
        }
        // Every other `(?` form -- `(?:`, `(?=`, `(?!`, `(?>`, `(?~`, `(?i)` and `(?i:` -- groups
        // without capturing.
        _ => 2,
    }
}
