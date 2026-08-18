//! `Layout/CommentIndentation`.

use std::ops::Range;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::support::{alignment_corrections, character_column, comments};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width: i64 = context
        .setting_of::<i64>("Layout/IndentationWidth", "Width")
        .unwrap_or(2);
    let outdent = context
        .setting_of::<String>("Layout/AccessModifierIndentation", "EnforcedStyle")
        .as_deref()
        == Some("outdent");
    let allow_for_alignment: bool = context.setting("AllowForAlignment").unwrap_or(false);

    let comments = comments(context);
    let columns: Vec<(usize, i64)> = comments
        .iter()
        .map(|comment| {
            (
                context.source.line_column(comment.start).0,
                character_column(context, comment.start),
            )
        })
        .collect();

    for (index, comment) in comments.iter().enumerate() {
        let (line, column) = columns[index];
        if !own_line_comment(context, line) {
            continue;
        }
        let next_line = line_after_comment(context, line);
        let mut correct = correct_indentation(next_line, width, outdent);
        let delta = correct - column;
        if delta == 0 {
            continue;
        }
        // A keyword that opens an alternative may be aligned either way, and the message names the
        // deeper of the two while the correction still aims at the shallower.
        if next_line.is_some_and(two_alternatives) {
            correct += width;
            if column == correct {
                continue;
            }
        }
        if allow_for_alignment && aligned_with_preceding_comment(context, &comments, index, column)
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!(
                        "Incorrect indentation detected (column {column} instead of {correct})."
                    ),
                    comment.clone(),
                )
                .corrected_by_all(chain_corrections(
                    context, &comments, &columns, index, delta,
                )),
        );
    }
}

/// `autocorrect_preceding_comments` followed by `autocorrect_one`: the comment itself, plus the run
/// of comments directly above it that start in the same column.
fn chain_corrections(
    context: &RuleContext<'_>,
    comments: &[Range<usize>],
    columns: &[(usize, i64)],
    index: usize,
    delta: i64,
) -> Vec<crate::diagnostic::Edit> {
    let mut edits = Vec::new();
    let mut below = index;
    while below > 0 {
        let above = below - 1;
        if columns[above].0 + 1 != columns[below].0 || columns[above].1 != columns[below].1 {
            break;
        }
        edits.extend(alignment_corrections(
            context,
            comments[above].clone(),
            delta,
            &[],
        ));
        below = above;
    }
    edits.extend(alignment_corrections(
        context,
        comments[index].clone(),
        delta,
        &[],
    ));
    edits
}

/// `own_line_comment?`: `/\A\s*#/` over the comment's own line.
fn own_line_comment(context: &RuleContext<'_>, line: usize) -> bool {
    context
        .source
        .line(line)
        .trim_start_matches([' ', '\t'])
        .starts_with('#')
}

/// `line_after_comment`: the first line below the comment that holds anything.
fn line_after_comment<'a>(context: &'a RuleContext<'_>, line: usize) -> Option<&'a str> {
    (line + 1..=context.source.line_count())
        .map(|line| {
            let text = context.source.line(line);
            crate::rules::support::chomp(text)
        })
        .find(|text| !text.trim().is_empty())
}

/// `correct_indentation`.
fn correct_indentation(next_line: Option<&str>, width: i64, outdent: bool) -> i64 {
    let Some(next_line) = next_line else {
        return 0;
    };
    let indentation = next_line
        .chars()
        .take_while(|character| character.is_whitespace())
        .count() as i64;
    indentation
        + if less_indented(next_line, outdent) {
            width
        } else {
            0
        }
}

/// `less_indented?`: the line closes something, so a comment above it belongs one level deeper.
fn less_indented(line: &str, outdent: bool) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with([')', '}', ']']) {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("end") {
        if !rest.starts_with(|character: char| character.is_alphanumeric() || character == '_') {
            return true;
        }
    }
    outdent && bare_access_modifier(trimmed)
}

/// `bare_access_modifier?`: the keyword alone on its line, which is what
/// `Layout/AccessModifierIndentation` outdents.
fn bare_access_modifier(trimmed: &str) -> bool {
    for keyword in ["private", "protected", "public"] {
        let Some(rest) = trimmed.strip_prefix(keyword) else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('#') {
            return true;
        }
    }
    false
}

/// `two_alternatives?`: `/^\s*(else|elsif|when|in|rescue|ensure)\b/`.
fn two_alternatives(line: &str) -> bool {
    let trimmed = line.trim_start();
    ["else", "elsif", "when", "in", "rescue", "ensure"]
        .iter()
        .any(|keyword| {
            trimmed.strip_prefix(keyword).is_some_and(|rest| {
                !rest.starts_with(|character: char| character.is_alphanumeric() || character == '_')
            })
        })
}

/// `correctly_aligned_with_preceding_comment?`: the nearest end-of-line comment above settles it.
fn aligned_with_preceding_comment(
    context: &RuleContext<'_>,
    comments: &[Range<usize>],
    index: usize,
    column: i64,
) -> bool {
    for other in comments[..index].iter().rev() {
        let line = context.source.line_column(other.start).0;
        if !own_line_comment(context, line) {
            return character_column(context, other.start) == column;
        }
    }
    false
}
