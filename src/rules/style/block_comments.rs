use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Do not use block comments.";
/// `"=begin\n".length`。最初の行の内容によらず、本家が取り除く先頭の文字数。
const BEGIN_LENGTH: usize = 7;
/// `"\n=end".length`。本家はコメントの末尾から測るので、`=end` の行の追加文が消す範囲を動かす。
const END_LENGTH: usize = 5;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for range in context.comment_ranges() {
        let comment = context.source.slice(range.clone());
        if !comment.starts_with("=begin") {
            continue;
        }
        // `=begin\n=end` はパーサが受理する最小のブロックで、これより短ければ構文診断で止まる。
        if range.len() < BEGIN_LENGTH + 4 {
            continue;
        }
        // `expr = comment.source_range`。`=end` の行の改行まで含み、改行の無い EOF で閉じると
        // whitequark では本文の 2 文字先で終わる。`=end` はこの終わりから測るので、補正もこの
        // 範囲で計算する。どちらの場合も、下の編集は本文の中に収まる。
        let expr = context.parser_comment_range(range);
        // SourceBuffer の位置は CRLF を 1 文字とする文字数。UTF-8 のバイト数で囲いを引くと、
        // `=end あいう` の本文を文字の途中で切り、CRLF では囲いの一部を残してしまう。
        let length = crate::diagnostic::character_length(context.source, range.start, range.end)
            + expr.end - range.end;
        let offset = |position| range.start + normalized_character_offset(comment, position);
        let begin_fence = expr.start..offset(BEGIN_LENGTH);
        // `comment.text.chomp == comment.text`。`comment.text` は本文に収まる部分の文字列。
        let end_fence = match comment.ends_with('\n') {
            true => offset(length - END_LENGTH)..offset(length),
            // chomp が何も変えない場合、本家は 1 文字前から始め、末尾の 2 文字を残す。
            false => offset(length - END_LENGTH - 1)..offset(length - 2),
        };
        // Prism で EOF に閉じた `=begin\n=end` は 2 つの囲いが逆になり、本家はその間に作る範囲で
        // 例外を上げて cop ごと落ちる。ファイルに何も報告しないので、ここでも何も報告しない。
        if end_fence.start < begin_fence.end {
            continue;
        }
        let contents = begin_fence.end..end_fence.start;

        let mut edits = vec![Edit {
            start: begin_fence.start,
            end: begin_fence.end,
            replacement: String::new(),
            safe: true,
        }];
        if !contents.is_empty() {
            edits.push(Edit {
                start: contents.start,
                end: contents.end,
                replacement: commented(&text[contents]),
                safe: true,
            });
        }
        edits.push(Edit {
            start: end_fence.start,
            end: end_fence.end,
            replacement: String::new(),
            safe: true,
        });
        offenses.push(context.comment_offense(MSG, range).corrected_by_all(edits));
    }
}

/// `source.gsub(/\A/, '# ').gsub("\n\n", "\n#\n").gsub(/\n(?=[^#])/, "\n# ")`.
///
/// The three substitutions run one after another over the whole string, and the second one's
/// output is what the third one reads: a run of blank lines is paired off from the left, and the
/// `#` the second put down then shields the newline before it from the third.
fn commented(source: &str) -> String {
    // 本家が置換へ渡す本文は LF に正規化済み。ディスクへ出す CRLF はエンジンが復元する。
    let source = source.replace("\r\n", "\n");
    let headed = format!("# {source}");
    let blanked = headed.replace("\n\n", "\n#\n");
    let bytes = blanked.as_bytes();
    let mut out = String::with_capacity(blanked.len());
    for (index, character) in blanked.char_indices() {
        out.push(character);
        if character == '\n' && bytes.get(index + 1).is_some_and(|next| *next != b'#') {
            out.push_str("# ");
        }
    }
    out
}

/// LF に正規化した文字位置を、元のコメントの UTF-8 バイト境界へ戻す。
/// CRLF の LF を飛ばし、改行の先頭は CR の前へ置く。
fn normalized_character_offset(comment: &str, position: usize) -> usize {
    let bytes = comment.as_bytes();
    comment
        .char_indices()
        .filter(|&(offset, character)| {
            character != '\n' || offset == 0 || bytes[offset - 1] != b'\r'
        })
        .nth(position)
        .map_or(comment.len(), |(offset, _)| offset)
}
