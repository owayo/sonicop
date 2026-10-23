use crate::diagnostic::{Offense, OffenseSnapshot};
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

    // 本家は末尾tokenの論理行までを検査する。commentやheredoc後のtNLは前の行へ戻る。
    // token無しの場合だけ生の行数へ戻り、__END__だけの入力も本家どおり検査する。
    let last_line = crate::rules::support::last_token_line(context)
        .unwrap_or_else(|| context.source.line_count());
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
        // 検査対象の最終行に改行自体が無い場合、本家もCRの欠落とは数えない。
        if crlf_expected && line_number == last_line && !line.ends_with('\n') {
            continue;
        }
        let message = if crlf_expected {
            "Carriage return character missing."
        } else {
            "Carriage return character detected."
        };
        // 本家は生の行長を CRLF 正規化済みの SourceBuffer に渡して範囲を作る。
        // CR が消えた分だけ終端が次の行へ進む場合がある。
        let range = context.source.line_range(line_number);
        let mut offense = context.offense(message, range);
        let mut location = offense.location(context.source);
        (location.last_line, location.last_column) =
            normalized_buffer_end(context.source.text(), offense.start, line.chars().count());
        location.length = line.chars().count();
        offense.snapshot = Some(OffenseSnapshot {
            location,
            source_line: line.to_owned(),
        });
        offenses.push(offense);
        // A file's line endings are almost always all alike, so RuboCop stops after the first.
        break;
    }
}

/// 生の行長を CRLF 正規化後のバッファへ適用したときの終端位置。
fn normalized_buffer_end(text: &str, start: usize, raw_length: usize) -> (usize, usize) {
    let prefix = &text[..start];
    let start_in_buffer = prefix.chars().count()
        - prefix
            .as_bytes()
            .windows(2)
            .filter(|pair| *pair == b"\r\n")
            .count();
    let end_in_buffer = start_in_buffer + raw_length;
    let mut line = 1;
    let mut column: usize = 1;
    let mut consumed = 0;
    let mut characters = text.chars().peekable();
    while consumed < end_in_buffer {
        let Some(character) = characters.next() else {
            column += end_in_buffer - consumed;
            break;
        };
        if character == '\r' && characters.peek() == Some(&'\n') {
            continue;
        }
        consumed += 1;
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column.saturating_sub(1).max(1))
}
