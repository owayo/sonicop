//! RuboCop 1.89.0 が TargetRubyVersion 2.7〜3.2 の構文を読むときの parser gem (`Parser::Ruby27` /
//! `Ruby30` / `Ruby31` / `Ruby32`、parser 3.3.12.0) を移したもの。`Lint/Syntax` の診断を作る。
//!
//! RuboCop は MRI では `all_errors_are_fatal = false` で parser gem を動かし、構文エラーの後も
//! racc の誤り回復で読み続けて診断を集める。どの診断が出てどれが出ないかは、文法の表・字句解析器・
//! 文法動作 (文脈やローカル変数の更新、builder の検査) と回復の手順の組み合わせで決まるので、
//! 推測で再現せずにその仕組みを移している。
//!
//! - `tables`: gem から読み出した racc と Ragel の表 (生成物)
//! - `parser`: racc の C 実装 (cparse) の駆動
//! - `lexer`: 字句解析器
//! - `actions` / `builder`: 文法動作と、診断に効く builder の処理

mod actions;
mod builder;
mod lexer;
mod parser;
mod regexp;
mod tables;
mod value;

use std::ops::Range;

use crate::ruby_version::RubyVersion;
use tables::Grammar;

/// parser gem の文法の版。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Version {
    Ruby27,
    Ruby30,
    Ruby31,
    Ruby32,
    Ruby33,
    Ruby34,
}

impl Version {
    /// RuboCop の `parser_class` (`parser_whitequark` の側)。3.3 以上は `ParserEngine: parser_whitequark`
    /// を指定したときだけ parser gem で読み、既定 (`default` / `parser_prism`) では Prism で読む。
    pub(crate) fn for_target(target: RubyVersion, whitequark_engine: bool) -> Option<Self> {
        let version = [
            (RubyVersion::new(2, 7), Self::Ruby27),
            (RubyVersion::new(3, 0), Self::Ruby30),
            (RubyVersion::new(3, 1), Self::Ruby31),
            (RubyVersion::new(3, 2), Self::Ruby32),
            (RubyVersion::new(3, 3), Self::Ruby33),
            (RubyVersion::new(3, 4), Self::Ruby34),
        ]
        .into_iter()
        .find_map(|(version, grammar)| (version == target).then_some(grammar))?;
        (version.number() < 33 || whitequark_engine).then_some(version)
    }

    pub(crate) fn number(self) -> u8 {
        match self {
            Self::Ruby27 => 27,
            Self::Ruby30 => 30,
            Self::Ruby31 => 31,
            Self::Ruby32 => 32,
            Self::Ruby33 => 33,
            Self::Ruby34 => 34,
        }
    }

    fn grammar(self) -> &'static Grammar {
        match self {
            Self::Ruby27 => &tables::ruby27::GRAMMAR,
            Self::Ruby30 => &tables::ruby30::GRAMMAR,
            Self::Ruby31 => &tables::ruby31::GRAMMAR,
            Self::Ruby32 => &tables::ruby32::GRAMMAR,
            Self::Ruby33 => &tables::ruby33::GRAMMAR,
            Self::Ruby34 => &tables::ruby34::GRAMMAR,
        }
    }
}

/// `Lint/Syntax` が報告する水準 (`error` と `fatal`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Error,
    Fatal,
}

/// parser gem の診断。`range` は CRLF を LF にした本文の文字単位の位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub(crate) level: Level,
    pub(crate) message: String,
    pub(crate) range: Range<usize>,
}

/// 本文を parser gem が読む形 (`Source::Buffer#source=`) にしたもの。
struct Source {
    points: Vec<u32>,
    /// 文字の添字ごとの、元の本文でのバイト位置 (末尾に本文の長さ)。
    offsets: Vec<usize>,
}

impl Source {
    /// `raw_source=` は CRLF を LF にする。位置はその後の本文の文字で数える。
    fn new(text: &str) -> Self {
        let mut points = Vec::with_capacity(text.len());
        let mut offsets = Vec::with_capacity(text.len() + 1);
        let mut chars = text.char_indices().peekable();
        while let Some((offset, c)) = chars.next() {
            if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
                chars.next();
                points.push(u32::from('\n'));
                offsets.push(offset);
                continue;
            }
            points.push(u32::from(c));
            offsets.push(offset);
        }
        offsets.push(text.len());
        Self { points, offsets }
    }

    fn byte_range(&self, range: &Range<usize>) -> Range<usize> {
        let at = |index: usize| self.offsets[index.min(self.offsets.len() - 1)];
        at(range.start)..at(range.end)
    }

    /// lexer の先読みが EOF を越えた分は表示だけに残し、本文の読取範囲とは分ける。
    fn diagnostic_range(&self, range: &Range<usize>) -> Range<usize> {
        let mut bytes = self.byte_range(range);
        bytes.end = bytes
            .end
            .saturating_add(range.end.saturating_sub(self.points.len()));
        bytes
    }
}

/// parser gem の診断を、生の順 (発生順) で返す。範囲は元の本文のバイト位置で、RuboCop の
/// `Lint/Syntax#diagnostic_location` のとおりゼロ幅を 1 文字へ広げてある。
/// EOF を越える終点は報告用なので、本文の切り出しや編集には渡さない。
pub(crate) fn diagnostics(text: &str, version: Version) -> Vec<Diagnostic> {
    let source = Source::new(text);
    let raw = parse(&source.points, version);
    let size = source.points.len();
    raw.into_iter()
        .map(|mut diagnostic| {
            let range = &mut diagnostic.range;
            if range.start == range.end {
                if range.end < size {
                    range.end += 1;
                } else if range.start > 0 {
                    range.start -= 1;
                }
            }
            diagnostic.range = source.diagnostic_range(range);
            diagnostic
        })
        .collect()
}

/// 診断を文字単位の位置のまま返す。
fn parse(points: &[u32], version: Version) -> Vec<Diagnostic> {
    let lexer = lexer::Lexer::new(i64::from(version.number()), points.to_vec());
    let mut parser = parser::Parser::new(version.grammar(), version, lexer);
    // fatal 診断で打ち切っても、それまでの診断は記録済み。
    let _ = parser.run();
    parser.diagnostics
}

/// 比較用のトークン列と診断文言。
pub type DebugOutput = (Vec<(String, usize, usize)>, Vec<String>);

/// 字句解析器が出したトークンの列 (比較用の道具が使う)。`(名前, 開始, 終了)` は文字単位。
pub fn debug_tokens(text: &str, target: RubyVersion) -> Option<DebugOutput> {
    let version = Version::for_target(target, true)?;
    let binary_text;
    let text = if crate::engine::declared_literal_encoding(text)
        == crate::engine::LiteralEncoding::Binary
    {
        binary_text = text
            .as_bytes()
            .iter()
            .map(|&byte| char::from(byte))
            .collect::<String>();
        binary_text.as_str()
    } else {
        text
    };
    let source = Source::new(text);
    let mut lexer = lexer::Lexer::new(i64::from(version.number()), source.points.clone());
    lexer.recorded = Some(Vec::new());
    let mut parser = parser::Parser::new(version.grammar(), version, lexer);
    let _ = parser.run();
    let tokens = parser
        .lexer
        .recorded
        .take()
        .unwrap_or_default()
        .into_iter()
        .map(|token| {
            (
                token.tok.name().to_owned(),
                token.range.start,
                token.range.end,
            )
        })
        .collect();
    let diagnostics = parser
        .diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{:?} {}-{} {}",
                diagnostic.level, diagnostic.range.start, diagnostic.range.end, diagnostic.message
            )
        })
        .collect();
    Some((tokens, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_lookahead_does_not_extend_source_reads() {
        for (text, range, read, reported) in [
            ("\"\\xx\"", 1..6, 1..5, 1..6),
            ("\"あ\\xx\"", 2..7, 4..8, 4..9),
            ("é\r\nx", 1..5, 2..5, 2..7),
        ] {
            let source = Source::new(text);
            assert_eq!(source.byte_range(&range), read);
            assert!(text.get(read).is_some());
            assert_eq!(source.diagnostic_range(&range), reported);
        }
    }

    #[test]
    fn invalid_hex_escape_keeps_the_recorded_whitequark_eof_end() {
        // RuboCop 1.89.0 の実 CLI は、5 byte の本文に対し文字範囲 1..6 を報告する。
        for version in [Version::Ruby33, Version::Ruby34] {
            let found = diagnostics("\"\\xx\"", version);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].message, "invalid hex escape");
            assert_eq!(found[0].range, 1..6);
        }
    }

    fn messages(source: &str, version: Version) -> Vec<(usize, usize, String)> {
        parse(&Source::new(source).points, version)
            .into_iter()
            .map(|diagnostic| {
                (
                    diagnostic.range.start,
                    diagnostic.range.end,
                    diagnostic.message,
                )
            })
            .collect()
    }

    /// racc の回復後、3 トークン shift するまで次の構文エラーは報告されない (owayo/sonicop#19)。
    #[test]
    fn errors_after_recovery_wait_for_three_shifted_tokens() {
        assert_eq!(
            messages("def a(&b) = nil\n", Version::Ruby27),
            vec![(10, 11, "unexpected token tEQL".to_owned())]
        );
        assert_eq!(
            messages("def foo(bar) = bar.length\n", Version::Ruby27),
            vec![
                (13, 14, "unexpected token tEQL".to_owned()),
                (26, 26, "unexpected token $end".to_owned()),
            ]
        );
        assert_eq!(
            messages("def a(&) = nil\n", Version::Ruby27),
            vec![(7, 8, "unexpected token tRPAREN".to_owned())]
        );
        assert!(messages("def a(&) = nil\n", Version::Ruby31).is_empty());
    }

    /// 回復で失われた def の `in_def` は戻らず、後の定数代入と class が意味エラーになる。
    #[test]
    fn a_lost_definition_leaks_its_context_into_the_rest_of_the_file() {
        let source =
            "class Foo\n  def a(&) = nil\n  def b(&) = nil\n  X = 1\n  class Bar\n  end\nend\n";
        let found: Vec<String> = messages(source, Version::Ruby27)
            .into_iter()
            .map(|(_, _, message)| message)
            .collect();
        assert_eq!(
            found,
            vec![
                "unexpected token tRPAREN",
                "unexpected token tRPAREN",
                "dynamic constant assignment",
                "class definition in method body",
            ]
        );
    }

    /// command 呼び出しの do ブロックは kDO_BLOCK で、括弧付き引数の中には書けない (owayo/sonicop#22)。
    #[test]
    fn a_do_block_on_a_command_inside_parenthesized_arguments_is_rejected() {
        assert_eq!(
            messages("puts(foo bar do\n  1\nend)\n", Version::Ruby27),
            vec![(13, 15, "unexpected token kDO_BLOCK".to_owned())]
        );
    }

    /// Buffer が UTF-8 へ復号した文字は Unicode escape、BINARY のままなら byte escape になる。
    #[test]
    fn unexpected_control_characters_follow_the_buffer_encoding() {
        for version in [
            Version::Ruby27,
            Version::Ruby30,
            Version::Ruby31,
            Version::Ruby32,
            Version::Ruby33,
            Version::Ruby34,
        ] {
            for (prefix, escapes) in [
                ("", ["\\u0001", "\\u000E", "\\u007F"]),
                ("# encoding: UTF-8\n", ["\\u0001", "\\u000E", "\\u007F"]),
                ("# encoding: US-ASCII\n", ["\\u0001", "\\u000E", "\\u007F"]),
                ("# encoding: ASCII-8BIT\n", ["\\x01", "\\x0E", "\\x7F"]),
                ("# encoding: BINARY\n", ["\\x01", "\\x0E", "\\x7F"]),
            ] {
                for (control, escaped) in ['\u{1}', '\u{e}', '\u{7f}'].into_iter().zip(escapes) {
                    let source = format!("{prefix}a=1\n{control}# kept\n");
                    let start = source.find(control).unwrap();
                    let found = diagnostics(&source, version);
                    assert_eq!(found.len(), 1, "{source:?} {version:?}");
                    assert_eq!(found[0].message, format!("unexpected `{escaped}'"));
                    assert_eq!(found[0].range, start..start + 1);
                    assert_eq!(found[0].level, Level::Fatal);
                }
            }
        }
    }

    /// Unicode の直後でも報告範囲は文字数でなく元 UTF-8 byte へ戻し、短い escape の綴りは保つ。
    #[test]
    fn unexpected_control_characters_keep_original_ranges_and_short_escapes() {
        for ending in ["\n", "\r\n"] {
            for (control, escaped) in [
                ('\u{1}', "\\u0001"),
                ('\u{7}', "\\a"),
                ('\u{8}', "\\b"),
                ('\u{1b}', "\\e"),
            ] {
                let source = format!("# encoding: UTF-8{ending}puts(\"あ\");{control}{ending}");
                let start = source.find(control).unwrap();
                let found = diagnostics(&source, Version::Ruby27);
                assert_eq!(found.len(), 1, "{source:?}");
                assert_eq!(found[0].message, format!("unexpected `{escaped}'"));
                assert_eq!(found[0].range, start..start + 1);
            }
        }
    }

    #[test]
    fn valid_code_draws_no_diagnostic() {
        for source in [
            "x = 1\nputs x\n",
            "foo(1, 2) { |a| a }\nbar do |b|\n  b\nend\n",
            "class A < B\n  def m(a, b = 1, *c, d:, e: 2, **f, &g)\n    yield\n  end\nend\n",
            "<<~EOS\n  a #{b}\nEOS\n",
            "case x\nwhen 1 then :a\nelse :b\nend\n",
            "a = %w[x y] + %i[z]\n/re#{1}/i =~ s\n",
        ] {
            assert!(messages(source, Version::Ruby27).is_empty(), "{source:?}");
        }
    }
}
