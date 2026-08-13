use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;

const MSG: &str = "Unnecessary utf-8 encoding comment.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.source.text().is_empty() {
        return;
    }
    for line_number in 1..=context.source.line_count() {
        let line = context
            .source
            .line(line_number)
            .trim_end_matches(['\n', '\r']);
        // A shebang is skipped rather than ending the run of magic comments.
        if line.starts_with("#!") {
            continue;
        }
        let comment = MagicComment::parse(line);
        // The first line that is not a magic comment ends the search, code or not.
        if !comment.valid(line) {
            return;
        }
        if !comment
            .encoding()
            .is_some_and(|encoding| encoding.eq_ignore_ascii_case("utf-8"))
        {
            continue;
        }
        let range = context.source.line_range(line_number);
        let range = range.start..range.start + line.len();
        let text = comment.without_encoding(line);
        // `blank?`: dropping the last setting leaves the line with nothing on it, so the line goes
        // too rather than being left empty.
        let edit = match text.is_empty() || text.trim_start().is_empty() {
            true => Edit {
                start: range.start,
                end: super::ranges::extended_right(context.source.text(), range.end, true),
                replacement: String::new(),
                safe: true,
            },
            false => Edit {
                start: range.start,
                end: range.end,
                replacement: text,
                safe: true,
            },
        };
        offenses.push(context.offense(MSG, range).corrected_by(edit));
    }
}
