//! `Parser::Lexer::Literal` (`lib/parser/lexer/literal.rb`)。文字列・正規表現・%記法・heredoc の
//! 1 つ分の状態で、閉じる位置の判定とトークンの出し方を受け持つ。
//!
//! トークンは呼び出し側の字句解析器が出す。ここでは出すトークンを `out` に積む。

use super::NIL;
use crate::engine::LiteralEncoding;
use crate::legacy_parser::tables::tokens::Tok;
use crate::legacy_parser::value::{Token, TokenValue};
use std::rc::Rc;

/// 開始の種類 (`TYPES`)。
fn literal_type(str_type: &str) -> Option<(Tok, bool)> {
    Some(match str_type {
        "'" | "<<'" | "%q" => (Tok::tSTRING_BEG, false),
        "\"" | "<<\"" | "%" | "%Q" => (Tok::tSTRING_BEG, true),
        "%w" => (Tok::tQWORDS_BEG, false),
        "%W" => (Tok::tWORDS_BEG, true),
        "%i" => (Tok::tQSYMBOLS_BEG, false),
        "%I" => (Tok::tSYMBOLS_BEG, true),
        ":'" | "%s" => (Tok::tSYMBEG, false),
        ":\"" => (Tok::tSYMBEG, true),
        "/" | "%r" => (Tok::tREGEXP_BEG, true),
        "%x" | "`" | "<<`" => (Tok::tXSTRING_BEG, true),
        _ => return None,
    })
}

fn closing_delimiter(delimiter: &str) -> Option<&'static str> {
    match delimiter {
        "(" => Some(")"),
        "[" => Some("]"),
        "{" => Some("}"),
        "<" => Some(">"),
        _ => None,
    }
}

fn token(tok: Option<Tok>, value: TokenValue, s: i64, e: i64) -> Token {
    Token {
        // 種類が分からない %記法の開始は `nil` のトークンで、構文解析器は `error` として読む。
        tok: tok.unwrap_or(Tok::ERROR),
        value,
        range: usize::try_from(s).unwrap_or(0)..usize::try_from(e).unwrap_or(0),
    }
}

pub(crate) struct Literal {
    nesting: i64,
    str_type: String,
    pub(crate) str_s: i64,
    start_tok: Option<Tok>,
    interpolate: bool,
    start_delim: Option<String>,
    end_delim: String,
    pub(crate) heredoc_e: Option<i64>,
    indent: bool,
    label_allowed: bool,
    dedent_body: bool,
    pub(crate) dedent_level: Option<i64>,
    interp_braces: i64,
    space_emitted: bool,
    monolithic: bool,
    buffer: Vec<u8>,
    buffer_encoding: LiteralEncoding,
    source_encoding: LiteralEncoding,
    buffer_s: Option<i64>,
    buffer_e: i64,
    pub(crate) saved_herebody_s: Option<i64>,
}

/// `Literal.new` が出す診断 (`unexpected_percent_str`)。
pub(crate) struct UnknownPercentString {
    pub(crate) str_type: String,
    pub(crate) str_s: i64,
}

impl Literal {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        str_type: &str,
        delimiter: &str,
        str_s: i64,
        heredoc_e: Option<i64>,
        indent: bool,
        dedent_body: bool,
        label_allowed: bool,
        encoding: LiteralEncoding,
        out: &mut Vec<Token>,
    ) -> (Self, Option<UnknownPercentString>) {
        let known = literal_type(str_type);
        let unknown = known.is_none().then(|| UnknownPercentString {
            str_type: str_type.to_owned(),
            str_s,
        });
        let (start_tok, interpolate) = match known {
            Some((tok, interpolate)) => (Some(tok), interpolate),
            None => (None, false),
        };
        let start_delim = closing_delimiter(delimiter).map(|_| delimiter.to_owned());
        let end_delim = closing_delimiter(delimiter).unwrap_or(delimiter).to_owned();
        let monolithic = start_tok == Some(Tok::tSTRING_BEG)
            && (str_type == "'" || str_type == "\"")
            && heredoc_e.is_none();
        let mut str_type = str_type.to_owned();
        if str_type.starts_with('%') {
            str_type.push_str(delimiter);
        }
        let literal = Self {
            nesting: 1,
            str_type,
            str_s,
            start_tok,
            interpolate,
            start_delim,
            end_delim,
            heredoc_e,
            indent,
            label_allowed,
            dedent_body,
            dedent_level: None,
            interp_braces: 0,
            space_emitted: true,
            monolithic,
            buffer: Vec::new(),
            buffer_encoding: encoding,
            source_encoding: encoding,
            buffer_s: None,
            buffer_e: NIL,
            saved_herebody_s: None,
        };
        if !literal.monolithic {
            literal.emit_start_tok(out);
        }
        (literal, unknown)
    }

    pub(crate) fn interpolate(&self) -> bool {
        self.interpolate
    }

    pub(crate) fn words(&self) -> bool {
        matches!(
            self.start_tok,
            Some(Tok::tWORDS_BEG | Tok::tQWORDS_BEG | Tok::tSYMBOLS_BEG | Tok::tQSYMBOLS_BEG)
        )
    }

    pub(crate) fn regexp(&self) -> bool {
        self.start_tok == Some(Tok::tREGEXP_BEG)
    }

    pub(crate) fn heredoc(&self) -> bool {
        self.heredoc_e.is_some()
    }

    pub(crate) fn squiggly_heredoc(&self) -> bool {
        self.heredoc() && self.dedent_body
    }

    pub(crate) fn backslash_delimited(&self) -> bool {
        self.end_delim == "\\"
    }

    pub(crate) fn start_tok(&self) -> Option<Tok> {
        self.start_tok
    }

    pub(crate) fn munge_escape(&self, character: &str) -> bool {
        if self.words() && matches!(character, " " | "\t" | "\u{b}" | "\r" | "\u{c}" | "\n") {
            return true;
        }
        character == "\\"
            || self.start_delim.as_deref() == Some(character)
            || self.end_delim == character
    }

    /// `nest_and_try_closing`: 閉じたら出したトークンの種類を返す。
    pub(crate) fn nest_and_try_closing(
        &mut self,
        delimiter: &str,
        ts: i64,
        te: i64,
        lookahead: Option<&str>,
        out: &mut Vec<Token>,
    ) -> Option<Tok> {
        if self.start_delim.as_deref() == Some(delimiter) {
            self.nesting += 1;
        } else if self.is_delimiter(delimiter) {
            self.nesting -= 1;
        }
        if self.nesting != 0 {
            return None;
        }
        if self.words() {
            self.extend_space(ts, ts, out);
        }
        let mut chars = lookahead.unwrap_or("").chars();
        let quoted_label = lookahead.is_some()
            && self.label_allowed
            && chars.next() == Some(':')
            && chars.next() != Some(':')
            && self.start_tok == Some(Tok::tSTRING_BEG);
        if quoted_label {
            self.flush_string(out);
            let end = self.end_delim.clone();
            out.push(token(
                Some(Tok::tLABEL_END),
                TokenValue::from(end.as_str()),
                ts,
                te + 1,
            ));
            Some(Tok::tLABEL_END)
        } else if self.monolithic {
            let buffer = self.buffer.clone();
            out.push(token(
                Some(Tok::tSTRING),
                TokenValue::Literal {
                    bytes: Rc::from(buffer),
                    encoding: self.buffer_encoding,
                },
                self.str_s,
                te,
            ));
            Some(Tok::tSTRING)
        } else {
            if !self.heredoc() {
                self.flush_string(out);
            }
            let end = self.end_delim.clone();
            out.push(token(
                Some(Tok::tSTRING_END),
                TokenValue::from(end.as_str()),
                ts,
                te,
            ));
            Some(Tok::tSTRING_END)
        }
    }

    pub(crate) fn infer_indent_level(&mut self, line: &str) {
        if !self.dedent_body {
            return;
        }
        let mut indent_level = 0;
        for c in line.chars() {
            match c {
                ' ' => indent_level += 1,
                '\t' => indent_level += 8 - indent_level % 8,
                _ => {
                    if self.dedent_level.is_none_or(|level| level > indent_level) {
                        self.dedent_level = Some(indent_level);
                    }
                    break;
                }
            }
        }
    }

    pub(crate) fn start_interp_brace(&mut self) {
        self.interp_braces += 1;
    }

    pub(crate) fn end_interp_brace_and_try_closing(&mut self) -> bool {
        self.interp_braces -= 1;
        self.interp_braces == 0
    }

    pub(crate) fn extend_string(&mut self, string: &str, ts: i64, te: i64) {
        if self.buffer_s.is_none() {
            self.buffer_s = Some(ts);
        }
        self.buffer_e = te;
        if self.source_encoding == LiteralEncoding::Binary {
            // binary の source_pts は生バイトを U+0000〜U+00FF として保持している。
            self.buffer
                .extend(string.chars().map(|character| character as u8));
        } else {
            self.buffer.extend_from_slice(string.as_bytes());
        }
    }

    /// ASCII だけの buffer に Unicode の値を足すと、Ruby の String と同じく UTF-8 になる。
    pub(crate) fn extend_bytes(
        &mut self,
        bytes: &[u8],
        encoding: LiteralEncoding,
        ts: i64,
        te: i64,
    ) {
        if self.buffer_s.is_none() {
            self.buffer_s = Some(ts);
        }
        self.buffer_e = te;
        if self.buffer.is_ascii() && !bytes.is_ascii() && encoding == LiteralEncoding::Text {
            self.buffer_encoding = LiteralEncoding::Text;
        }
        self.buffer.extend_from_slice(bytes);
    }

    pub(crate) fn flush_string(&mut self, out: &mut Vec<Token>) {
        if self.monolithic {
            self.emit_start_tok(out);
            self.monolithic = false;
        }
        if !self.buffer.is_empty() {
            let buffer = std::mem::take(&mut self.buffer);
            out.push(token(
                Some(Tok::tSTRING_CONTENT),
                TokenValue::Literal {
                    bytes: Rc::from(buffer),
                    encoding: self.buffer_encoding,
                },
                self.buffer_s.unwrap_or(NIL),
                self.buffer_e,
            ));
            self.clear_buffer();
            self.extend_content();
        }
    }

    pub(crate) fn extend_content(&mut self) {
        self.space_emitted = false;
    }

    pub(crate) fn extend_space(&mut self, ts: i64, te: i64, out: &mut Vec<Token>) {
        self.flush_string(out);
        if !self.space_emitted {
            out.push(token(Some(Tok::tSPACE), TokenValue::Nil, ts, te));
            self.space_emitted = true;
        }
    }

    pub(crate) fn supports_line_continuation_via_slash(&self) -> bool {
        !self.words() && self.interpolate
    }

    fn is_delimiter(&self, delimiter: &str) -> bool {
        if self.heredoc() {
            delimiter.ends_with(self.end_delim.as_str())
                && delimiter[..delimiter.len() - self.end_delim.len()]
                    .bytes()
                    .all(|c| c == b' ' || c == b'\t')
        } else if self.indent {
            self.end_delim == delimiter.trim_start()
        } else {
            self.end_delim == delimiter
        }
    }

    fn clear_buffer(&mut self) {
        self.buffer.clear();
        self.buffer_encoding = self.source_encoding;
        self.buffer_s = None;
        self.buffer_e = NIL;
    }

    fn emit_start_tok(&self, out: &mut Vec<Token>) {
        let str_e = self
            .heredoc_e
            .unwrap_or(self.str_s + self.str_type.len() as i64);
        out.push(token(
            self.start_tok,
            TokenValue::from(self.str_type.as_str()),
            self.str_s,
            str_e,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_tokens_keep_original_byte_values_without_unicode_substitution() {
        for (bytes, encoding) in [
            (&[0xc3, 0xa9][..], LiteralEncoding::Text),
            (&[0x80][..], LiteralEncoding::Binary),
        ] {
            let mut output = Vec::new();
            let (mut literal, _) = Literal::new(
                "\"",
                "\"",
                0,
                None,
                false,
                false,
                false,
                encoding,
                &mut output,
            );
            for byte in bytes {
                literal.extend_bytes(&[*byte], encoding, 1, 5);
            }
            literal.nest_and_try_closing("\"", 5, 6, None, &mut output);
            let TokenValue::Literal {
                bytes: actual,
                encoding: actual_encoding,
            } = &output[0].value
            else {
                panic!("文字列トークンにバイト列が無い");
            };
            assert_eq!(actual.as_ref(), bytes);
            assert_eq!(*actual_encoding, encoding);
        }
    }
}
