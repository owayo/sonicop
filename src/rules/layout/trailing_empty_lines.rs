use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    if text.is_empty() {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "final_newline".to_owned());
    let expected_newlines = usize::from(style == "final_blank_line") + 1;
    let without_newlines = text.trim_end_matches(['\r', '\n']);
    let actual_start = without_newlines.len();
    let actual = &text[actual_start..];
    // Only `\n` is counted, as RuboCop does. A carriage return is `Layout/EndOfLine`'s business;
    // reporting CRLF here as well would make the two cops rewrite the same bytes in opposite
    // directions on Windows, where the expected ending is CRLF.
    let newline_count = actual.bytes().filter(|byte| *byte == b'\n').count();
    if newline_count == expected_newlines {
        return;
    }
    let message = if newline_count < expected_newlines {
        "Final newline missing."
    } else {
        "Extra blank line detected at file end."
    };
    offenses.push(
        context
            .offense(message, actual_start.saturating_sub(1)..text.len())
            .corrected_by(Edit {
                start: actual_start,
                end: text.len(),
                replacement: "\n".repeat(expected_newlines),
                safe: true,
            }),
    );
}
