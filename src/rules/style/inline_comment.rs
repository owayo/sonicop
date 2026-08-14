use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Avoid trailing inline comments.";

/// A comment with code in front of it on the same line.
///
/// `comment_line?(processed_source[comment.loc.line - 1])` asks whether the line the comment sits on
/// is nothing but a comment, so what is left are the ones that trail code. A `rubocop:` directive is
/// configuration rather than prose and is exempt wherever it sits.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for range in context.comment_ranges() {
        let (line, _) = context.source.line_column(range.start);
        let text = context.source.line(line);
        if text.trim_start().starts_with('#') {
            continue;
        }
        let comment = &context.source.text()[range.clone()];
        if is_directive(comment) {
            continue;
        }
        offenses.push(context.offense(MSG, range.clone()));
    }
}

/// `/\A# rubocop:(enable|disable|todo)/`: anchored, and with exactly one space after the `#`.
fn is_directive(comment: &str) -> bool {
    let Some(rest) = comment.strip_prefix("# rubocop:") else {
        return false;
    };
    rest.starts_with("enable") || rest.starts_with("disable") || rest.starts_with("todo")
}
