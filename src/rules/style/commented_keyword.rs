//! `Style/CommentedKeyword`: a comment on the line of a keyword belongs above it.

use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Do not place comments on the same line as the `%<keyword>s` keyword.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for comment in context.comment_ranges() {
        let number = context.source.line_column(comment.start).0;
        let raw = context.source.line(number);
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let body = &text[comment.clone()];
        if !offensive(line, body) {
            continue;
        }
        let Some(keyword) = KEYWORD_BEFORE_COMMENT
            .captures(line)
            .and_then(|captures| captures.get(1))
            .map(|group| group.as_str())
        else {
            continue;
        };
        // `range_with_surrounding_space(newlines: false)`: the blanks on either side of the
        // comment go with it, but the line break does not.
        let mut start = comment.start;
        while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        let mut end = comment.end;
        while end < text.len() && matches!(text.as_bytes()[end], b' ' | b'\t') {
            end += 1;
        }
        let mut edits = vec![Edit {
            start,
            end,
            replacement: String::new(),
            safe: true,
        }];
        // An `end` has nothing the comment could be describing, so it is dropped rather than moved.
        if keyword != "end" {
            let line_start = context.source.line_start(number);
            edits.push(Edit {
                start: line_start,
                end: line_start,
                replacement: format!("{body}\n"),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(MSG.replace("%<keyword>s", keyword), comment.clone())
                .corrected_by_all(edits),
        );
    }
}

fn offensive(line: &str, comment: &str) -> bool {
    // `rbs_inline_annotation?`: an inline RBS annotation is not prose about the keyword.
    if SUBCLASS_DEFINITION.is_match(line) {
        if RBS_TYPE_APPLICATION.is_match(comment) {
            return false;
        }
    } else if METHOD_OR_END_DEFINITION.is_match(line) && comment.starts_with("#:") {
        return false;
    }
    if STEEP_IGNORE.is_match(comment) {
        return false;
    }
    KEYWORD.is_match(line) && !ALLOWED_COMMENT.is_match(line) && !is_rubocop_directive(line)
}

fn is_rubocop_directive(line: &str) -> bool {
    super::comments::is_rubocop_directive(line)
}

static KEYWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(?-u:\s)*(?:begin|class|def|end|module)(?-u:\s)").unwrap());

/// `REGEXP`: the first word of the line, which is the keyword the message names.
static KEYWORD_BEFORE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\S+).*#").unwrap());

static ALLOWED_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#(?-u:\s)*(?::nodoc:|:yields:)").unwrap());

static SUBCLASS_DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(?-u:\s)*class(?-u:\s)+(?:(?-u:\w)|::)+(?-u:\s)*<(?-u:\s)*(?:(?-u:\w)|::)+").unwrap());

static METHOD_OR_END_DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(?-u:\s)*(?:def(?-u:\s)|end)").unwrap());

static RBS_TYPE_APPLICATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A#\[.+\]").unwrap());

static STEEP_IGNORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#(?-u:\s)steep:ignore((?-u:\s)|\z)").unwrap());
