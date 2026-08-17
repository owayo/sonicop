//! `Layout/EmptyComment`.

use std::ops::Range;

use super::support::{comments as comment_ranges, whitespace_after};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Source code comment is empty.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let border = context
        .setting::<bool>("AllowBorderComment")
        .unwrap_or(true);
    let margin = context
        .setting::<bool>("AllowMarginComment")
        .unwrap_or(true);
    let comments = comment_ranges(context);

    if !margin {
        for comment in &comments {
            if is_empty(&[stripped(context, comment)], border) {
                offenses.push(report(context, comment));
            }
        }
        return;
    }
    // `concat_consecutive_comments`: a run of comments on consecutive lines at the same column is
    // one unit, so the empty lines that frame a description are not empty comments themselves.
    let mut index = 0;
    while index < comments.len() {
        let start = index;
        index += 1;
        while index < comments.len() && follows(context, &comments[index - 1], &comments[index]) {
            index += 1;
        }
        let chunk = &comments[start..index];
        let texts: Vec<&str> = chunk
            .iter()
            .map(|comment| stripped(context, comment))
            .collect();
        if is_empty(&texts, border) {
            offenses.extend(chunk.iter().map(|comment| report(context, comment)));
        }
    }
}

/// `i.loc.line.succ == j.loc.line && i.loc.column == j.loc.column`.
fn follows(context: &RuleContext<'_>, previous: &Range<usize>, next: &Range<usize>) -> bool {
    let (previous_line, previous_column) = context.source.line_column(previous.start);
    let (next_line, next_column) = context.source.line_column(next.start);
    previous_line + 1 == next_line && previous_column == next_column
}

/// `comment_text`: the comment's own text, with its trailing blanks taken off.
///
/// `String#strip` takes off NUL and the six ASCII whitespace characters and **leaves a no-break
/// space where it is**, so `#\u{a0}` is not an empty comment upstream. Rust's `trim` takes the whole
/// of `White_Space`, which turned that comment into a bare `#` and reported it as empty.
fn stripped<'a>(context: &'a RuleContext<'_>, comment: &Range<usize>) -> &'a str {
    context.source.text()[comment.clone()].trim_matches(crate::rules::support::is_ruby_strippable)
}

/// `/\A(#\n)+\z/`, or `/\A(#+\n)+\z/` once border comments are no longer allowed.
fn is_empty(texts: &[&str], allow_border_comment: bool) -> bool {
    !texts.is_empty()
        && texts.iter().all(|text| {
            !text.is_empty()
                && text.bytes().all(|byte| byte == b'#')
                && (!allow_border_comment || text.len() == 1)
        })
}

fn report(context: &RuleContext<'_>, comment: &Range<usize>) -> Offense {
    let text = context.source.text();
    let line = context.source.line_column(comment.start).0;
    let line_start = context.source.line_start(line);
    // `inline_comment?`: code sits ahead of the comment on its line, so only the comment and the
    // blanks around it go. A comment owning its line takes the whole line with it.
    let range = if text[line_start..comment.start].trim().is_empty() {
        let end = context.source.line_start(line + 1).max(comment.end);
        line_start..end
    } else {
        let mut start = comment.start;
        while start > line_start && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        start..whitespace_after(text, comment.end).end
    };
    context.offense(MSG, comment.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    })
}
