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
        // A `=begin` block is a comment too, and upstream's range for it takes in the newline
        // after `=end`.
        let reported = super::comments::parser_range(range, context);
        if is_directive(&context.source.text()[reported.clone()]) {
            continue;
        }
        offenses.push(context.offense(MSG, reported));
    }
}

/// `/\A# rubocop:(enable|disable|todo)/`: anchored, and with exactly one space after the `#`.
fn is_directive(comment: &str) -> bool {
    let Some(rest) = comment.strip_prefix("# rubocop:") else {
        return false;
    };
    rest.starts_with("enable") || rest.starts_with("disable") || rest.starts_with("todo")
}
