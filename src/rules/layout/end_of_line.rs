use crate::diagnostic::Offense;
use crate::rules::RuleContext;

/// Reports only, like RuboCop: this cop has no autocorrector upstream.
///
/// Rewriting line endings here would fight `Layout/TrailingEmptyLines`, which normalizes the end
/// of the file to `\n`. On Windows, where `native` means CRLF, the two would undo each other on
/// every pass and autocorrect would never settle.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "native".to_owned());
    let crlf_expected = style == "crlf" || (style == "native" && cfg!(windows));
    let bytes = context.source.text().as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let has_cr = index > 0 && bytes[index - 1] == b'\r';
        if has_cr == crlf_expected {
            continue;
        }
        let (start, end) = if crlf_expected {
            (index, index)
        } else {
            (index - 1, index + 1)
        };
        let message = if crlf_expected {
            "Carriage return character missing."
        } else {
            "Carriage return character detected."
        };
        offenses.push(context.offense(message, start..end));
    }
}
