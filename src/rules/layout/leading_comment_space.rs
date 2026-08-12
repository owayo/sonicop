//! `Layout/LeadingCommentSpace`.

use std::collections::HashSet;
use std::ops::Range;

use super::support::comments;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Missing space after `#`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    let file_name = context
        .source
        .path()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let doxygen = context
        .setting::<bool>("AllowDoxygenCommentStyle")
        .unwrap_or(false);
    let gemfile_ruby = context
        .setting::<bool>("AllowGemfileRubyComment")
        .unwrap_or(false);
    let rbs_inline = context
        .setting::<bool>("AllowRBSInlineAnnotation")
        .unwrap_or(false);
    let steep = context
        .setting::<bool>("AllowSteepAnnotation")
        .unwrap_or(false);
    let yard_separator = context
        .setting::<bool>("AllowYARDCommentBlockSeparator")
        .unwrap_or(false);

    let mut reported: HashSet<usize> = HashSet::new();
    let all = comments(context);
    for comment in &all {
        let body = &text[comment.clone()];
        if !missing_space(body) {
            continue;
        }
        let line = context.source.line_column(comment.start).0;
        let shebang = body.starts_with("#!");
        if line == 1 && (shebang || (file_name == "config.ru" && body.starts_with("#\\"))) {
            continue;
        }
        // A second `#!` line continues a shebang that was itself allowed.
        if shebang && previous_shebang_was_allowed(context, line, &reported) {
            continue;
        }
        if doxygen && body.starts_with("#*") {
            continue;
        }
        if gemfile_ruby && file_name == "Gemfile" && body.starts_with("#ruby") {
            continue;
        }
        if rbs_inline && is_rbs_inline(body) {
            continue;
        }
        if steep && (body.starts_with("#$") || body.starts_with("#:")) {
            continue;
        }
        if yard_separator && body.starts_with("#-") && body[2..].trim().is_empty() {
            continue;
        }
        reported.insert(comment.start);
        offenses.push(
            context
                .offense(MSG, comment.clone())
                .corrected_by(Edit {
                    start: comment.start + 1,
                    end: comment.start + 1,
                    replacement: " ".to_owned(),
                    safe: true,
                })
                // `insert_after(hash_mark(expr), ' ')`: the anchor is the `#` alone rather than the
                // whole comment the offense was reported on.
                .corrections_anchored_at(comment.start..(comment.start + 1)),
        );
    }
}

/// `/\A(?!#\+\+|#--)(#+[^#\s=])/`.
fn missing_space(body: &str) -> bool {
    if body.starts_with("#++") || body.starts_with("#--") {
        return false;
    }
    let hashes = body.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 {
        return false;
    }
    body[hashes..]
        .chars()
        .next()
        .is_some_and(|character| character != '=' && !character.is_whitespace())
}

/// `#:` / `#[...]` / `#|`.
fn is_rbs_inline(body: &str) -> bool {
    body.starts_with("#:")
        || body.starts_with("#|")
        || (body.starts_with("#[") && body[2..].contains(']'))
}

/// `shebang_continuation?`: the comment one line up is a shebang this run let through.
fn previous_shebang_was_allowed(
    context: &RuleContext<'_>,
    line: usize,
    reported: &HashSet<usize>,
) -> bool {
    if line <= 1 {
        return true;
    }
    let Some(previous) = comment_at_line(context, line - 1) else {
        return false;
    };
    context.source.text()[previous.clone()].starts_with("#!") && !reported.contains(&previous.start)
}

fn comment_at_line(context: &RuleContext<'_>, line: usize) -> Option<Range<usize>> {
    context
        .comment_ranges()
        .iter()
        .find(|comment| context.source.line_column(comment.start).0 == line)
        .cloned()
}
