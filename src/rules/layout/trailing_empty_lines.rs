use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::support;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    if text.is_empty() {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "final_newline".to_owned());
    let wanted_blank_lines = isize::from(style == "final_blank_line");

    // All trailing whitespace, as RuboCop's `/\s*\Z/` takes it -- not just newlines, so a file
    // ending in spaces is measured from where the whitespace starts rather than from the last
    // newline. Only `\n` is counted, leaving carriage returns to `Layout/EndOfLine`.
    // **`char::is_whitespace` would reach over a no-break space**, which `/\s/` does not match:
    // a last line holding one is not trailing whitespace, and counting it as such reports a blank
    // line upstream does not see -- and this cop's correction would then delete that line.
    let whitespace_start = text
        .trim_end_matches(support::is_ruby_space_char)
        .len();
    let whitespace = &text[whitespace_start..];
    let blank_lines = whitespace.matches('\n').count() as isize - 1;
    if blank_lines == wanted_blank_lines {
        return;
    }

    let message = match blank_lines {
        -1 => "Final newline missing.".to_owned(),
        0 => "Trailing blank line missing.".to_owned(),
        count if wanted_blank_lines == 0 => format!("{count} trailing blank lines detected."),
        count => format!("{count} trailing blank lines instead of {wanted_blank_lines} detected."),
    };

    // The offense starts one byte into the trailing whitespace, so that it points at the first
    // line that should not be there rather than at the last line of real code. With no trailing
    // whitespace at all it collapses to the end of the file, which is where the missing newline
    // belongs. The correction still covers the whitespace in full.
    let report_start = if whitespace.is_empty() {
        text.len()
    } else {
        whitespace_start + 1
    };
    offenses.push(
        context
            .offense(message, report_start..text.len())
            .corrected_by(Edit {
                start: whitespace_start,
                end: text.len(),
                replacement: "\n".repeat((wanted_blank_lines + 1) as usize),
                safe: true,
            }),
    );
}
