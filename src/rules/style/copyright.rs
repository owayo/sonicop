use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `/\A(?:\\A|\^)?#(?:\\s[*+?]?|\s)*/`, the comment marker `notice_regexp` takes off the configured
/// notice before compiling it.
static COMMENT_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(?:\\A|\^)?#(?:\\s[*+?]?|(?-u:\s))*").unwrap());

/// `/\A# */`, which each comment loses before it joins the notice being built.
static LEADING_HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A# *").unwrap());

/// A copyright notice must come before any code.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let notice = context.setting::<String>("Notice").unwrap_or_default();
    if notice.is_empty() {
        return;
    }
    let Some(pattern) = notice_regexp(&notice) else {
        return;
    };
    if notice_found(context, &pattern) {
        return;
    }
    let message = format!("Include a copyright notice matching /{notice}/ before any code.");
    // `processed_source.blank?`: a file with nothing but comments has no AST upstream, and the
    // offense it gets is a global one.
    let range = if has_code(context) { 0..1 } else { 0..0 };
    offenses.push(context.offense(message, range));
}

/// `notice_regexp`.
///
/// Ruby anchors `^` to the start of a line, which the notice is matched against line by line, so the
/// pattern is compiled in multi-line mode.
fn notice_regexp(notice: &str) -> Option<Regex> {
    let pattern = COMMENT_PREFIX.replace(notice, "");
    RegexBuilder::new(&pattern).multi_line(true).build().ok()
}

/// `notice_found?`: the comments that come before any code, joined, with each one's `# ` taken off.
/// The walk stops at the first comment that matches on its own.
fn notice_found(context: &RuleContext<'_>, pattern: &Regex) -> bool {
    let mut joined = String::new();
    for range in leading_comments(context) {
        let text = &context.source.text()[range.clone()];
        joined.push_str(&LEADING_HASH.replace(text, ""));
        joined.push('\n');
        if pattern.is_match(text) {
            break;
        }
    }
    pattern.is_match(&joined)
}

/// The comments written before the first token that is not one.
fn leading_comments(context: &RuleContext<'_>) -> Vec<std::ops::Range<usize>> {
    let limit = first_code_offset(context).unwrap_or(usize::MAX);
    context
        .comment_ranges()
        .iter()
        .take_while(|range| range.start < limit)
        .cloned()
        .collect()
}

/// Whether the file holds anything but comments.
fn has_code(context: &RuleContext<'_>) -> bool {
    first_code_offset(context).is_some()
}

fn first_code_offset(context: &RuleContext<'_>) -> Option<usize> {
    super::nodes::children(context.root_node())
        .into_iter()
        .find(|child| child.kind_str() != "comment")
        .map(|child| child.start_byte())
}
