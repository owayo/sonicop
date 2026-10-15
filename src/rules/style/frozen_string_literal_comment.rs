use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::layout::tokens::{Token, TokenKind};

const MSG_MISSING: &str = "Missing frozen string literal comment.";
const MSG_MISSING_TRUE: &str = "Missing magic comment `# frozen_string_literal: true`.";
const MSG_UNNECESSARY: &str = "Unnecessary frozen string literal comment.";
const MSG_DISABLED: &str = "Frozen string literal comment must be set to `true`.";

/// 本家の `minimum_target_ruby_version 2.3` に合わせる。
const MINIMUM: RubyVersion = RubyVersion::new(2, 3);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    // 本家はトークンのないファイルを調べない。BOM だけの入力やデータ節だけの入力も含む。
    if without_initial_bom(crate::rules::support::code_before_data(context), 1)
        .trim()
        .is_empty()
    {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "always".to_owned());

    match style.as_str() {
        "never" => {
            // 本家の `ensure_no_comment` は先頭のコメントだけで有効性を判定する。
            // コードの後ろの宣言は Ruby に届かないため、不要な宣言としては削除しない。
            // 診断位置には、別途 `specified_comment` が見つけた最初の宣言を使う。
            let leading = leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).valid_literal_value());
            if !leading {
                return;
            }
            if let Some(comment) = specified_comment(context) {
                offenses.push(
                    context
                        .offense(MSG_UNNECESSARY, comment.clone())
                        .corrected_by(remove_comment(context, comment)),
                );
            }
        }
        "always_true" => {
            // false の宣言は欠落と区別し、ファイル先頭ではなくコメント上に報告する。
            if !leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).frozen_string_literal_specified())
            {
                offenses.push(missing_offense(context, MSG_MISSING_TRUE));
            } else if !leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).frozen_string_literal_enabled())
                && let Some(comment) = specified_comment(context)
            {
                let line = context.source.line_column(comment.start).0;
                let replacement_range =
                    context.source.line_start(line)..line_content_end(context, line);
                offenses.push(
                    context
                        .offense(MSG_DISABLED, comment.clone())
                        .corrected_by(Edit {
                            start: replacement_range.start,
                            end: replacement_range.end,
                            // 本家は定型コメントで行全体を置換するため、BOM や字下げは残さない。
                            replacement: enabled_form(context.source.slice(comment.clone())),
                            safe: false,
                        })
                        .corrections_anchored_at(replacement_range),
                );
            }
        }
        // 本家は Ruby が認識する true / false だけを数えるため、yes は宣言の欠落になる。
        _ => {
            if !leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).valid_literal_value())
            {
                offenses.push(missing_offense(context, MSG_MISSING));
            }
        }
    }
}

fn missing_offense(context: &RuleContext<'_>, message: &str) -> Offense {
    // 本家はファイル先頭の 1 文字に欠落を報告し、挿入先だけを shebang・符号化宣言の後ろへ動かす。
    // 補正のアンカーは診断範囲とは異なり、直前の宣言行かバッファ全体になる。
    // 他 cop の編集との包含関係が変わるため、この範囲を明示する。
    let (edit, anchor) = match last_special_comment_line(context) {
        // 元の行末文字を、新しく挿入する宣言行の行末として使う。
        Some(line_number) => {
            let end = line_content_end(context, line_number);
            (
                Edit {
                    start: end,
                    end,
                    replacement: "\n# frozen_string_literal: true".to_owned(),
                    safe: false,
                },
                context.source.line_start(line_number)..end,
            )
        }
        None => (
            Edit {
                start: 0,
                end: 0,
                replacement: "# frozen_string_literal: true\n".to_owned(),
                safe: false,
            },
            0..context.source.text().len(),
        ),
    };
    // 本家の長さは文字単位なので、3 バイトの BOM も先頭の 1 文字として範囲に含める。
    let first_character = context
        .source
        .text()
        .chars()
        .next()
        .map_or(1, char::len_utf8);
    context
        .offense(message, 0..first_character)
        .corrected_by(edit)
        .corrections_anchored_at(anchor)
}

/// 本家はコメントに限定せず、先頭トークンの値で挿入先を決める。
fn last_special_comment_line(context: &RuleContext<'_>) -> Option<usize> {
    let mut tokens = context.layout_tokens().iter();
    let first = tokens.next()?;
    let mut special = None;
    let candidate = if token_text(first, context).starts_with(b"#!") {
        special = Some(first.line);
        // 本家は後続コメントを tNL より先に返す。次がコードなら文字列後の改行で区切る。
        tokens.next().filter(|next| {
            first.is_comment()
                || next.is_comment()
                || !context.source.slice(first.range.end..next.range.start)
                    .split_inclusive('\n')
                    .any(|part| part.ends_with('\n') && !part.ends_with("\\\n") && !part.ends_with("\\\r\n"))
        })
    } else {
        Some(first)
    };
    if let Some(token) = candidate
        && is_utf8_encoding_comment(&token_text(token, context))
    {
        special = Some(token.line);
    }
    special
}

fn token_text<'a>(token: &Token, context: &'a RuleContext<'_>) -> std::borrow::Cow<'a, [u8]> {
    // tSTRING の text は引用符を含まず、エスケープも解決済み。
    // %q や補間付きリテラルの開始トークンは文字列全体とは区別する。
    let raw = context.source.slice(token.range.clone());
    if (token.kind == TokenKind::String
        || (token.kind == TokenKind::Other && raw.starts_with('%')))
        && let Some(node) = context
            .nodes_of("string")
            .find(|node| node.byte_range() == token.range)
    {
        if token.kind == TokenKind::String {
            return crate::rules::literal::node_bytes(context, node)
                .unwrap_or_else(|| raw.as_bytes().to_vec())
                .into();
        }
        // 共有列は percent 文字列を一つの Other にまとめる場合があるが、
        // 本家の先頭トークンは %q{ などの開始区切りだけを持つ。
        if let Some(opener) = context.children(node).and_then(|mut children| children.next()) {
            return context.source.node_text(opener).as_bytes().into();
        }
    }
    raw.as_bytes().into()
}

fn is_utf8_encoding_comment(text: &[u8]) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"#.*coding(?-u:\s)?[:=](?-u:\s)?(?:UTF|utf)-8")
            .expect("encoding comment pattern must compile")
    });
    // 本家の Token#text.valid_encoding? を保つ。\xFF を Unicode の文字に変換しない。
    std::str::from_utf8(text).is_ok_and(|line| PATTERN.is_match(line))
}

/// 本家の `line_range` と同じく、改行の手前で範囲を止める。
fn line_content_end(context: &RuleContext<'_>, line_number: usize) -> usize {
    let range = context.source.line_range(line_number);
    range.start
        + context
            .source
            .line(line_number)
            .trim_end_matches(['\n', '\r'])
            .len()
}

/// 本家の side: :right と同じく、コメントより前の BOM や空白は消さない。
fn remove_comment(context: &RuleContext<'_>, comment: Range<usize>) -> Edit {
    let line_number = context.source.line_column(comment.start).0;
    let start = comment.start;
    // コメントの後ろの改行と空白を含め、削除後に先頭の空行を残さない。
    let text = context.source.text();
    let mut end = context.source.line_range(line_number).end;
    while text[end..]
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
    {
        end += text[end..].chars().next().map_or(0, char::len_utf8);
    }
    Edit {
        start,
        end,
        replacement: String::new(),
        safe: false,
    }
}

/// 本家は最初の非コメントトークンで区切るが、判定には BOM を含む元の行を渡す。
/// BOM 付きの宣言行で打ち切ると、後続のコメントを毎パス追加し続けてしまう。
fn leading_comments<'a>(
    context: &'a RuleContext<'_>,
) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    let first_code = (1..=context.source.line_count()).find(|line_number| {
        let line = without_initial_bom(context.source.line(*line_number), *line_number).trim();
        !line.is_empty() && !line.starts_with('#')
    });
    let end = first_code.unwrap_or(context.source.line_count() + 1);
    (1..end).map(|line_number| (line_number, context.source.line(line_number)))
}

fn without_initial_bom(line: &str, line_number: usize) -> &str {
    if line_number == 1 {
        line.strip_prefix('\u{feff}').unwrap_or(line)
    } else {
        line
    }
}

/// 本家が削除・置換する最初のコメントトークンを、BOM を含まない範囲で返す。
fn specified_comment(context: &RuleContext<'_>) -> Option<Range<usize>> {
    (1..=context.source.line_count()).find_map(|line_number| {
        let line = context.source.line(line_number);
        let token_line = without_initial_bom(line, line_number);
        // コメントトークンは末尾の空白も含むため、診断範囲から削ってはいけない。
        let trimmed = token_line.trim_start().trim_end_matches(['\r', '\n']);
        if !trimmed.starts_with('#')
            || !MagicComment::parse(trimmed).frozen_string_literal_specified()
        {
            return None;
        }
        let start =
            context.source.line_start(line_number) + (line.len() - token_line.trim_start().len());
        Some(start..start + trimmed.len())
    })
}

/// 本家の new_frozen_string_literal は、Emacs / 通常コメントそれぞれの定型へ置き換える。
fn enabled_form(comment: &str) -> String {
    match MagicComment::parse(comment) {
        MagicComment::Emacs(_) => "# -*- frozen_string_literal: true -*-".to_owned(),
        _ => "# frozen_string_literal: true".to_owned(),
    }
}
