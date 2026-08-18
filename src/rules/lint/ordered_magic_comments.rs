use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;

use super::statements::statements;

const MSG: &str = "The encoding magic comment should precede all other magic comments.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.source.is_empty() {
        return;
    }
    let (Some(encoding), Some(other)) = magic_comment_lines(context) else {
        return;
    };
    if encoding < other {
        return;
    }
    let first = line_range(encoding, context);
    let second = line_range(other, context);
    offenses.push(context.offense(MSG, first.clone()).corrected_by_all([
        Edit {
            start: first.start,
            end: first.end,
            replacement: context.source.slice(second.clone()).to_owned(),
            safe: true,
        },
        Edit {
            start: second.start,
            end: second.end,
            replacement: context.source.slice(first).to_owned(),
            safe: true,
        },
    ]));
}

/// The first line that declares an encoding and the first that is any other magic comment, both as
/// 1-based line numbers. Only the lines before the first line of code are considered.
fn magic_comment_lines(context: &RuleContext<'_>) -> (Option<usize>, Option<usize>) {
    let mut lines = (None, None);
    for line_number in 1..=leading_lines(context) {
        let line = context.source.slice(line_range(line_number, context));
        let comment = MagicComment::parse(line);
        if comment.encoding().is_some() {
            lines.0 = lines.0.or(Some(line_number));
        // `valid?` is `@comment.start_with?('#') && any?`, so an indented magic comment is not one.
        } else if line.starts_with('#') && comment.any() {
            lines.1 = lines.1.or(Some(line_number));
        }
        if lines.0.is_some() && lines.1.is_some() {
            return lines;
        }
    }
    lines
}

/// `buffer.line_range`, which stops before the line separator rather than taking it along.
fn line_range(line_number: usize, context: &RuleContext<'_>) -> std::ops::Range<usize> {
    let range = context.source.line_range(line_number);
    let text = context.source.slice(range.clone());
    let stripped = crate::rules::support::chomp(text);
    let stripped = stripped.strip_suffix('\r').unwrap_or(stripped);
    range.start..range.start + stripped.len()
}

/// `leading_comment_lines`: every line before the one the first token that is not a comment stands
/// on, which is the whole file when it holds no code at all.
fn leading_lines(context: &RuleContext<'_>) -> usize {
    match statements(context.root_node()).first() {
        Some(first) => context.source.line_column(first.start_byte()).0 - 1,
        None => context.source.line_count(),
    }
}
