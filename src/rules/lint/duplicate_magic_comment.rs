use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;

use super::statements::statements;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.source.is_empty() {
        return;
    }
    let mut encodings: Vec<usize> = Vec::new();
    let mut frozen: Vec<usize> = Vec::new();
    for line_number in 1..=leading_lines(context) {
        let line = context.source.slice(line_range(line_number, context));
        let comment = MagicComment::parse(line);
        if comment.encoding().is_some() {
            encodings.push(line_number);
        } else if comment.frozen_string_literal_specified() {
            frozen.push(line_number);
        }
    }
    for lines in [encodings, frozen] {
        for &line_number in lines.iter().skip(1) {
            let range = line_range(line_number, context);
            let whole = context.source.line_range(line_number);
            offenses.push(
                context
                    .offense("Duplicate magic comment detected.", range)
                    .corrected_by(Edit {
                        start: whole.start,
                        end: whole.end,
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
    }
}

/// `buffer.line_range`, which stops before the line separator rather than taking it along.
fn line_range(line_number: usize, context: &RuleContext<'_>) -> Range<usize> {
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
