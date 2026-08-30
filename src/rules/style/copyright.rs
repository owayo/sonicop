use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::diagnostic::{Edit, Offense};
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
    let offense = context.offense(message, range);
    // `autocorrect`: the notice is written in before the first token that is neither a shebang nor
    // an encoding comment. `verify_autocorrect_notice!` refuses one the pattern does not match, so
    // a misconfigured notice raises rather than being inserted -- here it simply corrects nothing.
    let autocorrect_notice = context
        .setting::<String>("AutocorrectNotice")
        .unwrap_or_default();
    let normalized = normalized_notice(&autocorrect_notice);
    // `verify_autocorrect_notice!` matches `autocorrect_notice.gsub(/^#\s*/, '')` -- the notice is
    // checked **without** its comment marker, because the pattern is written against the text.
    let bare: String = normalized
        .lines()
        .map(|line| line.trim_start().trim_start_matches('#').trim_start())
        .collect::<Vec<_>>()
        .join("\n");
    if autocorrect_notice.is_empty() || !pattern.is_match(&bare) {
        offenses.push(offense);
        return;
    }
    let at = insert_notice_before(context);
    offenses.push(offense.corrected_by(Edit {
        start: at,
        end: at,
        replacement: format!("{normalized}\n"),
        safe: false,
    }));
}

/// `normalized_autocorrect_notice`: every line becomes a comment, and a blank one becomes `#`.
fn normalized_notice(notice: &str) -> String {
    notice
        .lines()
        .map(|line| match () {
            () if line.starts_with('#') => line.to_owned(),
            () if line.trim().is_empty() => "#".to_owned(),
            () => format!("# {line}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `insert_notice_before`: after a shebang and an encoding comment, and before everything else.
fn insert_notice_before(context: &RuleContext<'_>) -> usize {
    let mut line = 1;
    if context
        .source
        .line(line)
        .trim_start()
        .starts_with("#!")
    {
        line += 1;
    }
    if line <= context.source.line_count()
        && context.source.line(line).contains("coding")
        && context.source.line(line).trim_start().starts_with('#')
    {
        line += 1;
    }
    match line {
        1 => 0,
        _ => context.source.line_start(line),
    }
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
    super::nodes::children_in(context.root_node(), context)
        .into_iter()
        .find(|child| child.kind_str() != "comment")
        .map(|child| child.start_byte())
}
