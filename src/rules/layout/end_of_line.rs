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

    // `last_line`: the line the **last token** sits on, not the last line of the file. Everything
    // past `__END__` is `DATA` and holds no tokens, so its line endings are none of this cop's
    // business -- scanning to the end of the file reported the data section instead.
    let line_count = context.source.line_count();
    let last_line = context
        .nodes_of("uninterpreted")
        .next()
        .map_or(line_count, |node| {
            context
                .source
                .line_column(node.start_byte())
                .0
                .saturating_sub(1)
        });
    for line_number in 1..=last_line {
        let line = context.source.line(line_number);
        let has_crlf = line.ends_with("\r\n");
        let offending = if crlf_expected {
            !has_crlf
        } else {
            has_crlf || line.ends_with('\r')
        };
        if !offending {
            continue;
        }
        // A last line with no line terminator at all cannot be missing a carriage return.
        if crlf_expected && line_number == line_count && !line.ends_with('\n') {
            continue;
        }
        let message = if crlf_expected {
            "Carriage return character missing."
        } else {
            "Carriage return character detected."
        };
        // The offense covers the whole line, terminator included, rather than just the carriage
        // return: RuboCop builds it as `source_range(buffer, line, 0, line.length)`.
        let range = context.source.line_range(line_number);
        offenses.push(context.offense(message, range));
        // A file's line endings are almost always all alike, so RuboCop stops after the first.
        break;
    }
}
