//! `Layout/SpaceInsideHashLiteralBraces`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// The neighbour of a brace, as the token stream presents it: where it begins, and what it is.
struct Neighbour {
    start: usize,
    /// The offset the token before a brace ends at, which is where the blanks before the brace
    /// begin.
    end: usize,
    comment: bool,
    right_curly: bool,
    left_brace: bool,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "space".to_owned());
    let empty_no_space = context
        .setting::<String>("EnforcedStyleForEmptyBraces")
        .as_deref()
        .unwrap_or("no_space")
        == "no_space";
    let text = context.source.text();

    for node in context.nodes_of_any(&["hash", "hash_pattern"]) {
        let count = node.child_count();
        let (Some(open), Some(close)) = (
            node.child(0),
            node.child(u32::try_from(count).unwrap_or(0).saturating_sub(1)),
        ) else {
            continue;
        };
        // `tokens.first.left_brace? && tokens.last.right_curly_brace?`: a brace-less hash has no
        // delimiters to have anything inside of.
        if open.kind() != "{" || close.kind() != "}" || open.end_byte() > close.start_byte() {
            continue;
        }
        let mut reported: Vec<(Range<usize>, String, Edit)> = Vec::new();

        // `check(tokens[0], tokens[1])`: the brace and whatever follows it.
        let following = next_token(text, open.end_byte());
        push(
            context,
            &mut reported,
            &open,
            &following,
            &style,
            empty_no_space,
            true,
        );
        // `check(tokens[-2], tokens[-1])`, skipped for `{}` and `{ }`, whose two tokens are all
        // there is.
        if following.start < close.start_byte() {
            let preceding = previous_token(text, close.start_byte());
            push(
                context,
                &mut reported,
                &preceding,
                &close,
                &style,
                empty_no_space,
                false,
            );
        }
        // `check_whitespace_only_hash`: braces holding nothing but blanks, including a line break,
        // which the token pair above cannot see.
        if empty_no_space {
            let inside = open.end_byte()..close.start_byte();
            if !inside.is_empty() && text[inside.clone()].trim().is_empty() {
                reported.push((
                    inside.clone(),
                    "Space inside empty hash literal braces detected.".to_owned(),
                    Edit {
                        start: inside.start,
                        end: inside.end,
                        replacement: String::new(),
                        safe: true,
                    },
                ));
            }
        }
        // `add_offense` keeps a set of the ranges it has already reported, so the whitespace-only
        // check and the brace pair never report the same span twice.
        let mut seen: Vec<Range<usize>> = Vec::new();
        for (range, message, edit) in reported {
            if seen.contains(&range) {
                continue;
            }
            seen.push(range.clone());
            offenses.push(context.offense(message, range).corrected_by(edit));
        }
    }
}

/// `check`, then `incorrect_style_detected`.
#[allow(clippy::too_many_arguments)]
fn push(
    context: &RuleContext<'_>,
    reported: &mut Vec<(Range<usize>, String, Edit)>,
    left: &impl Brace,
    right: &impl Brace,
    style: &str,
    empty_no_space: bool,
    opening: bool,
) {
    // A line break inside the braces, which a trailing comment also stands for, leaves nothing to
    // measure.
    if context.source.line_column(left.start()).0 < context.source.line_column(right.start()).0
        || right.is_comment()
    {
        return;
    }
    let empty_braces = left.is_left_brace() && right.is_right_curly();
    let expect_space = if left.is_left_brace() == right.is_left_brace()
        && left.is_right_curly() == right.is_right_curly()
        && style == "compact"
    {
        false
    } else if empty_braces {
        !empty_no_space
    } else {
        style != "no_space"
    };
    let has_space = left.end() < right.start();
    if has_space == expect_space {
        return;
    }
    let text = context.source.text();
    let brace = match opening {
        true => left.start()..left.end(),
        false => right.start()..right.end(),
    };
    let range = match expect_space {
        true => brace.clone(),
        // `space_range`: the blanks the brace is padded with, on the side that faces inwards.
        false => match opening {
            true => (brace.start + 1)..spaces_after(text, brace.end),
            false => spaces_before(text, brace.start)..(brace.end - 1),
        },
    };
    let inside_what = match empty_braces {
        true => "empty hash literal braces".to_owned(),
        false => text[brace].to_owned(),
    };
    let problem = match expect_space {
        true => "missing",
        false => "detected",
    };
    // `insert_after` for the opening brace and `insert_before` for the closing one, both hanging
    // off the brace the offense reports; a padded brace has its padding removed instead.
    let edit = match expect_space {
        false => Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        },
        true => {
            let offset = match opening {
                true => range.end,
                false => range.start,
            };
            Edit {
                start: offset,
                end: offset,
                replacement: " ".to_owned(),
                safe: true,
            }
        }
    };
    reported.push((
        range,
        format!("Space inside {inside_what} {problem}."),
        edit,
    ));
}

/// What `check` asks of the two tokens it compares.
trait Brace {
    fn start(&self) -> usize;
    fn end(&self) -> usize;
    fn is_comment(&self) -> bool;
    fn is_right_curly(&self) -> bool;
    fn is_left_brace(&self) -> bool;
}

impl Brace for Node<'_> {
    fn start(&self) -> usize {
        self.start_byte()
    }
    fn end(&self) -> usize {
        self.end_byte()
    }
    fn is_comment(&self) -> bool {
        false
    }
    fn is_right_curly(&self) -> bool {
        self.kind() == "}"
    }
    fn is_left_brace(&self) -> bool {
        self.kind() == "{"
    }
}

impl Brace for Neighbour {
    fn start(&self) -> usize {
        self.start
    }
    fn end(&self) -> usize {
        self.end
    }
    fn is_comment(&self) -> bool {
        self.comment
    }
    fn is_right_curly(&self) -> bool {
        self.right_curly
    }
    fn is_left_brace(&self) -> bool {
        self.left_brace
    }
}

/// The token after `offset`, which begins at the next character that is not blank -- whitespace is
/// not a token, so nothing else can lie between.
fn next_token(text: &str, offset: usize) -> Neighbour {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start < bytes.len() && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    Neighbour {
        start,
        end: start,
        comment: bytes.get(start) == Some(&b'#'),
        right_curly: bytes.get(start) == Some(&b'}'),
        left_brace: bytes.get(start) == Some(&b'{'),
    }
}

/// The token before `offset`, which ends at the last character that is not blank.
fn previous_token(text: &str, offset: usize) -> Neighbour {
    let bytes = text.as_bytes();
    let mut end = offset;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    Neighbour {
        // A token before a closing brace never spans lines: whatever closes it -- a quote, a
        // bracket, a heredoc opener -- is the last thing on that line of the token.
        start: end.saturating_sub(1),
        end,
        comment: false,
        right_curly: end > 0 && bytes[end - 1] == b'}',
        left_brace: end > 0 && bytes[end - 1] == b'{',
    }
}

fn spaces_after(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = offset;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    end
}

fn spaces_before(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start
}
