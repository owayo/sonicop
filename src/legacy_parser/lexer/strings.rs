//! `Parser::LexerStrings` (`lexer-strings.rl`)。文字列・正規表現・%記法・heredoc の中身を読む
//! もう 1 つの Ragel の字句解析器。遷移表は `tables/lexer_strings.rs` の生成物で、動作は数が少なく
//! 大半が同じ本体の繰り返しなので、生成物 (`lib/parser/lexer-strings.rb`) の `case` を手で写した。

use super::literal::Literal;
use super::{L_AGAIN, L_EOF_TRANS, L_OUT, L_RESUME, L_TEST_EOF, Lexer, NIL, slice_points};
use crate::engine::LiteralEncoding;
use crate::legacy_parser::Level;
use crate::legacy_parser::parser::Fatal;
use crate::legacy_parser::tables::lexer as main_tables;
use crate::legacy_parser::tables::lexer_strings::*;
use crate::legacy_parser::tables::tokens::Tok;
use crate::legacy_parser::value::{Token, TokenValue};
use std::rc::Rc;

/// 文字列側の字句解析器の状態 (`LexerStrings` のインスタンス変数)。
pub(crate) struct Strings {
    pub(crate) cs: i64,
    pub(crate) literal_stack: Vec<Literal>,
    escape_s: i64,
    escape: Option<(Vec<u8>, LiteralEncoding)>,
    pub(crate) herebody_s: Option<i64>,
    pub(crate) dedent_level: Option<i64>,
    root_lexer_state: i64,
    ts: i64,
    te: i64,
}

impl Strings {
    pub(crate) fn new() -> Self {
        Self {
            cs: EN_UNKNOWN as i64,
            literal_stack: Vec::new(),
            escape_s: NIL,
            escape: None,
            herebody_s: None,
            dedent_level: None,
            root_lexer_state: NIL,
            ts: NIL,
            te: NIL,
        }
    }
}

/// 遷移に付いた動作の本体のうち、繰り返し現れるもの。
enum Body {
    InterpCode,
    StringEol,
    String,
    InterpDigitVar,
    InterpVar,
    StringEscaped,
    StringSpace,
}

/// `fbreak` で動作の並びから抜けたこと。
struct Break;

impl Lexer {
    fn s_tok(&self, s: i64, e: i64) -> String {
        slice_points(&self.src, s, e - s)
    }

    /// LexerStrings#emit (既定の範囲は文字列側の `@ts`・`@te`)。
    fn s_emit(&mut self, tok: Tok, value: TokenValue, s: i64, e: i64) {
        self.emit(tok, value, s, e);
    }

    fn flush(&mut self, out: Vec<Token>) {
        for token in out {
            if let Some(recorded) = self.recorded.as_mut() {
                recorded.push(token.clone());
            }
            self.token_queue.push_back(token);
        }
    }

    fn s_diagnostic(&mut self, level: Level, message: &str, (s, e): (i64, i64)) {
        self.diagnostic(level, message.to_owned(), (s, e));
    }

    fn s_range(&self) -> (i64, i64) {
        (self.strings.ts, self.strings.te)
    }

    /// `@strings.advance(p)`: `[p, @root_lexer_state]`。
    pub(crate) fn strings_advance(&mut self, p: i64) -> Result<(i64, i64), Fatal> {
        let p = self.exec_strings(p)?;
        Ok((p, self.strings.root_lexer_state))
    }

    pub(crate) fn read_character_constant(&mut self, p: i64) -> Result<(i64, i64), Fatal> {
        self.strings.cs = EN_CHARACTER as i64;
        self.strings_advance(p)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_literal(
        &mut self,
        str_type: String,
        delimiter: String,
        str_s: i64,
        heredoc_e: Option<i64>,
        indent: bool,
        dedent_body: bool,
        label_allowed: bool,
    ) {
        let mut out = Vec::new();
        let (literal, unknown) = Literal::new(
            &str_type,
            &delimiter,
            str_s,
            heredoc_e,
            indent,
            dedent_body,
            label_allowed,
            self.encoding,
            &mut out,
        );
        if let Some(unknown) = unknown {
            // 診断は開始トークンより先に出る (Literal#initialize の順)。
            self.diagnostic(
                Level::Error,
                format!("{}: unknown type of percent-literal", unknown.str_type),
                (unknown.str_s, unknown.str_s + 2),
            );
        }
        self.flush(out);
        self.strings.cs = next_state_for_literal(&literal);
        self.strings.literal_stack.push(literal);
    }

    fn continue_lexing(&mut self) {
        if let Some(literal) = self.strings.literal_stack.last() {
            self.strings.cs = next_state_for_literal(literal);
        }
    }

    /// `pop_literal`: 正規表現なら修飾子を読む状態を返す。
    fn pop_literal(&mut self) -> Option<i64> {
        let old = self.strings.literal_stack.pop()?;
        self.strings.dedent_level = old.dedent_level;
        if old.start_tok() == Some(Tok::tREGEXP_BEG) {
            self.strings.root_lexer_state = main_tables::EN_INSIDE_STRING as i64;
            Some(EN_REGEXP_MODIFIERS as i64)
        } else {
            self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
            None
        }
    }

    pub(crate) fn close_interp_on_current_literal(&mut self, p: i64) -> bool {
        let Some(literal) = self.strings.literal_stack.last_mut() else {
            return false;
        };
        if !literal.end_interp_brace_and_try_closing() {
            return false;
        }
        let saved = literal.saved_herebody_s;
        self.emit(Tok::tSTRING_DEND, TokenValue::from("}"), p - 1, p);
        if let Some(saved) = saved {
            self.strings.herebody_s = Some(saved);
        }
        self.continue_lexing();
        true
    }

    fn eof_codepoint_at(&self, p: i64) -> bool {
        // `source_pts[p]` が範囲外なら nil で、どの値とも一致しない。
        usize::try_from(p)
            .ok()
            .and_then(|index| self.src.get(index))
            .is_some_and(|&point| matches!(point, 0x04 | 0x1a | 0x00))
    }

    fn emit_invalid_escapes(&self) -> bool {
        if self.version < 32 {
            return true;
        }
        match self.strings.literal_stack.last() {
            None => true,
            Some(literal) => !literal.regexp(),
        }
    }

    fn with_literal(&mut self, f: impl FnOnce(&mut Literal, &mut Vec<Token>)) {
        let mut out = Vec::new();
        if let Some(literal) = self.strings.literal_stack.last_mut() {
            f(literal, &mut out);
        }
        self.flush(out);
    }

    fn extend_string_escaped(&mut self) {
        let (ts, te) = (self.strings.ts, self.strings.te);
        let escaped_char = self.s_tok(self.strings.escape_s, self.strings.escape_s + 1);
        let tok = self.s_tok(ts, te);
        let escape = self.strings.escape.clone();
        let version = self.version;
        self.with_literal(|literal, _| {
            if literal.munge_escape(&escaped_char) {
                if literal.regexp()
                    && escaped_char
                        .chars()
                        .any(|c| "\\$()*+.<>?[]^{|}".contains(c))
                {
                    literal.extend_string(&tok, ts, te);
                } else {
                    literal.extend_string(&escaped_char, ts, te);
                }
            } else if literal.squiggly_heredoc() && escaped_char == "\n" {
                literal.extend_string(&tok, ts, te);
            } else if literal.supports_line_continuation_via_slash() && escaped_char == "\n" {
                literal.extend_string(&tok.replace("\\\n", ""), ts, te);
            } else if literal.regexp()
                && version >= 31
                && matches!(escaped_char.as_str(), "c" | "C" | "m" | "M")
            {
                if let Some((bytes, encoding)) = &escape {
                    literal.extend_bytes(bytes, *encoding, ts, te);
                }
            } else if literal.regexp() {
                literal.extend_string(&tok.replace("\\\n", ""), ts, te);
            } else {
                if let Some((bytes, encoding)) = &escape {
                    literal.extend_bytes(bytes, *encoding, ts, te);
                } else {
                    literal.extend_string(&tok, ts, te);
                }
            }
        });
    }

    fn extend_interp_code(&mut self) {
        self.with_literal(|literal, out| {
            literal.flush_string(out);
            literal.extend_content();
        });
        let (ts, te) = self.s_range();
        self.s_emit(Tok::tSTRING_DBEG, TokenValue::from("#{"), ts, te);
        let herebody_s = self.strings.herebody_s;
        let mut clear = false;
        if let Some(literal) = self.strings.literal_stack.last_mut() {
            if literal.heredoc() {
                literal.saved_herebody_s = herebody_s;
                clear = true;
            }
            literal.start_interp_brace();
        }
        if clear {
            self.strings.herebody_s = None;
        }
        self.command_start = true;
    }

    fn extend_interp_digit_var(&mut self) {
        if self.version >= 27 {
            let (ts, te) = self.s_range();
            let tok = self.s_tok(ts, te);
            self.with_literal(|literal, _| literal.extend_string(&tok, ts, te));
        } else {
            let (ts, te) = self.s_range();
            let tok = self.s_tok(ts, te);
            let name = self.s_tok(ts + 1, te);
            let message = if tok.starts_with("#@@") {
                format!("`{name}' is not allowed as a class variable name")
            } else {
                format!("`{name}' is not allowed as an instance variable name")
            };
            self.diagnostic(Level::Error, message, (ts + 1, te));
        }
    }

    fn extend_string_eol_check_eof(&mut self, pe: i64) -> Result<(), Fatal> {
        if self.strings.te == pe {
            let str_s = self
                .strings
                .literal_stack
                .last()
                .map_or(NIL, |literal| literal.str_s);
            self.s_diagnostic(
                Level::Fatal,
                "unterminated string meets end of file",
                (str_s, str_s + 1),
            );
            return Err(Fatal);
        }
        Ok(())
    }

    fn extend_string_eol_heredoc_line(&self) -> String {
        let line = self.s_tok(self.strings.herebody_s.unwrap_or(NIL), self.strings.ts);
        line.trim_end_matches('\r').to_owned()
    }

    fn extend_string_eol_heredoc_intertwined(&mut self, p: i64) -> i64 {
        match self.strings.herebody_s.take() {
            Some(herebody_s) => herebody_s - 1,
            None => p,
        }
    }

    fn extend_string_eol_words(&mut self, p: i64) {
        let (ts, te) = self.s_range();
        let tok = self.s_tok(ts, te);
        let eof = self.eof_codepoint_at(p);
        self.with_literal(|literal, out| {
            if literal.words() && !eof {
                literal.extend_space(ts, te, out);
            } else {
                literal.extend_string(&tok, ts, te);
                literal.flush_string(out);
            }
        });
    }

    fn extend_string_slice_end(&self, lookahead: Option<String>) -> Option<String> {
        if self.version >= 22 && !self.cond.is_active() {
            Some(self.s_tok(self.strings.te, self.strings.te + 2))
        } else {
            lookahead
        }
    }

    /// `Integer#chr` は byte escape を UTF-8 の文字へ変換しない。
    fn encode_escape(&self, ord: u32) -> (Vec<u8>, LiteralEncoding) {
        (vec![ord as u8], self.encoding)
    }

    fn unescape_char(&mut self, p: i64) -> Result<(), Fatal> {
        let codepoint = self.point(p - 1);
        if self.version >= 30 && (codepoint == 117 || codepoint == 85) {
            let range = self.s_range();
            self.s_diagnostic(Level::Fatal, "invalid escape character syntax", range);
            return Err(Fatal);
        }
        let escape = match char::from_u32(codepoint) {
            Some('a') => "\u{7}".to_owned(),
            Some('b') => "\u{8}".to_owned(),
            Some('e') => "\u{1b}".to_owned(),
            Some('f') => "\u{c}".to_owned(),
            Some('n') => "\n".to_owned(),
            Some('r') => "\r".to_owned(),
            Some('s') => " ".to_owned(),
            Some('t') => "\t".to_owned(),
            Some('v') => "\u{b}".to_owned(),
            Some('\\') => "\\".to_owned(),
            _ => self.s_tok(p - 1, p),
        };
        self.strings.escape = Some((escape.into_bytes(), self.encoding));
        Ok(())
    }

    fn unicode_points(&mut self, p: i64) {
        let mut escape = String::new();
        let codepoints = self.s_tok(self.strings.escape_s + 2, p - 1);
        let mut codepoint_s = self.strings.escape_s + 2;
        // `scan(/([0-9a-fA-F]+)|([ \t]+)/)` の順に読む。
        let chars: Vec<char> = codepoints.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            let start = index;
            if chars[index].is_ascii_hexdigit() {
                while index < chars.len() && chars[index].is_ascii_hexdigit() {
                    index += 1;
                }
                let digits: String = chars[start..index].iter().collect();
                let length = (index - start) as i64;
                let codepoint = u32::from_str_radix(&digits, 16).unwrap_or(u32::MAX);
                if codepoint >= 0x110000 {
                    self.s_diagnostic(
                        Level::Error,
                        "invalid Unicode codepoint (too large)",
                        (codepoint_s, codepoint_s + length),
                    );
                    break;
                }
                if codepoint & 0xfffff800 == 0xd800 {
                    self.s_diagnostic(
                        Level::Error,
                        "invalid Unicode escape",
                        (codepoint_s, codepoint_s + length),
                    );
                    break;
                }
                if let Some(c) = char::from_u32(codepoint) {
                    escape.push(c);
                }
                codepoint_s += length;
            } else if chars[index] == ' ' || chars[index] == '\t' {
                while index < chars.len() && (chars[index] == ' ' || chars[index] == '\t') {
                    index += 1;
                }
                codepoint_s += (index - start) as i64;
            } else {
                index += 1;
            }
        }
        self.strings.escape = Some((escape.into_bytes(), LiteralEncoding::Text));
    }

    fn read_post_meta_or_ctrl_char(&mut self, p: i64) -> Result<(), Fatal> {
        let escape = self.s_tok(p - 1, p);
        let ord = escape.chars().next().map_or(0, u32::from);
        self.strings.escape = Some((escape.into_bytes(), self.encoding));
        if self.version >= 27 && ((0..=8).contains(&ord) || (14..=31).contains(&ord)) {
            let range = self.s_range();
            self.s_diagnostic(Level::Fatal, "invalid escape character syntax", range);
            return Err(Fatal);
        }
        Ok(())
    }

    fn escape_ord(&self) -> u32 {
        self.strings
            .escape
            .as_ref()
            .and_then(|(bytes, encoding)| {
                if *encoding == LiteralEncoding::Text {
                    std::str::from_utf8(bytes)
                        .ok()
                        .and_then(|text| text.chars().next())
                        .map(u32::from)
                        .or_else(|| bytes.first().copied().map(u32::from))
                } else {
                    bytes.first().copied().map(u32::from)
                }
            })
            .unwrap_or(0)
    }

    fn check_invalid_escapes(&mut self, p: i64) -> Result<(), Fatal> {
        if self.emit_invalid_escapes() {
            let s = self.strings.escape_s - 1;
            self.s_diagnostic(Level::Fatal, "invalid Unicode escape", (s, p));
            return Err(Fatal);
        }
        Ok(())
    }

    fn emit_character_constant(&mut self) {
        let (ts, te) = self.s_range();
        let (bytes, encoding) = self
            .strings
            .escape
            .clone()
            .unwrap_or_else(|| (self.s_tok(ts + 1, te).into_bytes(), self.encoding));
        self.s_emit(
            Tok::tCHARACTER,
            TokenValue::Literal {
                bytes: Rc::from(bytes),
                encoding,
            },
            ts,
            te,
        );
    }

    /// 本体の動作。`Err(Ok(Break))` は fbreak、`Err(Err(Fatal))` は fatal 診断。
    fn run_body(
        &mut self,
        body: Body,
        p: &mut i64,
        pe: i64,
        lookahead: &mut Option<String>,
        interp_var_kind: &str,
    ) -> Result<(), Result<Break, Fatal>> {
        match body {
            Body::InterpCode => {
                self.extend_interp_code();
                self.strings.root_lexer_state = main_tables::EN_EXPR_VALUE as i64;
                *p += 1;
                Err(Ok(Break))
            }
            Body::StringEol => {
                self.extend_string_eol_check_eof(pe).map_err(Err)?;
                let heredoc = self
                    .strings
                    .literal_stack
                    .last()
                    .is_some_and(Literal::heredoc);
                if heredoc {
                    let line = self.extend_string_eol_heredoc_line();
                    let herebody_s = self.strings.herebody_s.unwrap_or(NIL);
                    let ts = self.strings.ts;
                    let mut out = Vec::new();
                    let closed = self.strings.literal_stack.last_mut().and_then(|literal| {
                        literal.nest_and_try_closing(&line, herebody_s, ts, None, &mut out)
                    });
                    self.flush(out);
                    if closed.is_some() {
                        self.strings.herebody_s = Some(self.strings.te);
                        let heredoc_e = self
                            .strings
                            .literal_stack
                            .last()
                            .and_then(|literal| literal.heredoc_e)
                            .unwrap_or(NIL);
                        *p = heredoc_e - 1;
                        self.strings.cs = self.pop_literal().unwrap_or(NIL);
                        *p += 1;
                        return Err(Ok(Break));
                    }
                    if let Some(literal) = self.strings.literal_stack.last_mut() {
                        literal.infer_indent_level(&line);
                    }
                    self.strings.herebody_s = Some(self.strings.te);
                } else {
                    let (ts, te) = self.s_range();
                    let tok = self.s_tok(ts, te);
                    let mut out = Vec::new();
                    let closed = self.strings.literal_stack.last_mut().and_then(|literal| {
                        literal.nest_and_try_closing(&tok, ts, te, None, &mut out)
                    });
                    self.flush(out);
                    if closed.is_some() {
                        self.strings.cs = self.pop_literal().unwrap_or(NIL);
                        *p += 1;
                        return Err(Ok(Break));
                    }
                    *p = self.extend_string_eol_heredoc_intertwined(*p);
                }
                self.extend_string_eol_words(*p);
                Ok(())
            }
            Body::String => {
                let (ts, te) = self.s_range();
                let string = self.s_tok(ts, te);
                *lookahead = self.extend_string_slice_end(lookahead.take());
                let heredoc = self
                    .strings
                    .literal_stack
                    .last()
                    .is_some_and(Literal::heredoc);
                let mut out = Vec::new();
                let token = if heredoc {
                    None
                } else {
                    self.strings.literal_stack.last_mut().and_then(|literal| {
                        literal.nest_and_try_closing(
                            &string,
                            ts,
                            te,
                            lookahead.as_deref(),
                            &mut out,
                        )
                    })
                };
                self.flush(out);
                match token {
                    Some(tok) => {
                        if tok == Tok::tLABEL_END {
                            *p += 1;
                            self.pop_literal();
                            self.strings.root_lexer_state = main_tables::EN_EXPR_LABELARG as i64;
                        } else if let Some(state) = self.pop_literal() {
                            self.strings.cs = state;
                        }
                        *p += 1;
                        Err(Ok(Break))
                    }
                    None => {
                        self.with_literal(|literal, _| literal.extend_string(&string, ts, te));
                        Ok(())
                    }
                }
            }
            Body::InterpDigitVar => {
                self.extend_interp_digit_var();
                Ok(())
            }
            Body::InterpVar => {
                let (ts, te) = self.s_range();
                self.with_literal(|literal, out| {
                    literal.flush_string(out);
                    literal.extend_content();
                });
                self.s_emit(Tok::tSTRING_DVAR, TokenValue::Nil, ts, ts + 1);
                match interp_var_kind {
                    "cvar" => self.emit_class_var_at(ts + 1, te),
                    "ivar" => self.emit_instance_var_at(ts + 1, te),
                    "gvar" => self.emit_global_var_at(ts + 1, te),
                    _ => {}
                }
                Ok(())
            }
            Body::StringEscaped => {
                self.extend_string_escaped();
                Ok(())
            }
            Body::StringSpace => {
                let (ts, te) = self.s_range();
                self.with_literal(|literal, out| literal.extend_space(ts, te, out));
                Ok(())
            }
        }
    }

    /// `LexerStrings#advance` の `%% write exec;` の部分。
    fn exec_strings(&mut self, mut p: i64) -> Result<i64, Fatal> {
        let pe = self.src.len() as i64 + 2;
        let eof = pe;
        let mut goto_level = 0;
        let mut trans: usize = 0;
        let mut lookahead: Option<String> = None;
        let mut interp_var_kind = "";
        loop {
            let mut trigger_goto = false;
            if goto_level <= 0 {
                if p == pe {
                    goto_level = L_TEST_EOF;
                    continue;
                }
                if self.strings.cs == 0 {
                    goto_level = L_OUT;
                    continue;
                }
            }
            if goto_level <= L_RESUME {
                let acts = usize::from(FROM_STATE_ACTIONS[self.strings.cs as usize]);
                for &action in action_list(acts) {
                    if action == 24 {
                        self.strings.ts = p;
                    }
                }
                trans = self.strings_transition(p);
            }
            if goto_level <= L_EOF_TRANS {
                self.strings.cs = i64::from(TRANS_TARGS[trans]);
                let acts = usize::from(TRANS_ACTIONS[trans]);
                if acts != 0 {
                    for &action in action_list(acts) {
                        match self.run_action(
                            action,
                            &mut p,
                            pe,
                            &mut lookahead,
                            &mut interp_var_kind,
                        ) {
                            Ok(()) => {}
                            Err(Ok(Break)) => {
                                trigger_goto = true;
                                goto_level = L_OUT;
                                break;
                            }
                            Err(Err(fatal)) => return Err(fatal),
                        }
                    }
                    if trigger_goto {
                        continue;
                    }
                }
            }
            if goto_level <= L_AGAIN {
                let acts = usize::from(TO_STATE_ACTIONS[self.strings.cs as usize]);
                for &action in action_list(acts) {
                    if action == 23 {
                        self.strings.ts = NIL;
                    }
                }
                if self.strings.cs == 0 {
                    goto_level = L_OUT;
                    continue;
                }
                p += 1;
                if p != pe {
                    goto_level = L_RESUME;
                    continue;
                }
            }
            if goto_level <= L_TEST_EOF && p == eof && EOF_TRANS[self.strings.cs as usize] > 0 {
                trans = usize::from(EOF_TRANS[self.strings.cs as usize]) - 1;
                goto_level = L_EOF_TRANS;
                continue;
            }
            break;
        }
        Ok(p)
    }

    fn strings_transition(&self, p: i64) -> usize {
        let cs = self.strings.cs as usize;
        let keys = cs << 1;
        let inds = usize::from(INDEX_OFFSETS[cs]);
        let slen = usize::from(KEY_SPANS[cs]);
        let wide = i64::from(self.point(p));
        let low = i64::from(TRANS_KEYS[keys]);
        let high = i64::from(TRANS_KEYS[keys + 1]);
        let index = if slen > 0 && low <= wide && wide <= high {
            inds + (wide - low) as usize
        } else {
            inds + slen
        };
        usize::from(INDICIES[index])
    }

    /// `_lex_actions` の 1 つ (`case _lex_actions[_acts - 1]`)。
    fn run_action(
        &mut self,
        action: u16,
        p: &mut i64,
        pe: i64,
        lookahead: &mut Option<String>,
        interp_var_kind: &mut &'static str,
    ) -> Result<(), Result<Break, Fatal>> {
        let fatal = |lexer: &mut Self, message: &str, range: (i64, i64)| {
            lexer.s_diagnostic(Level::Fatal, message, range);
            Err(Err(Fatal))
        };
        match action {
            0 => {}
            1 => self.unicode_points(*p),
            2 => self.unescape_char(*p).map_err(Err)?,
            3 => {
                let range = self.s_range();
                return fatal(self, "invalid escape character syntax", range);
            }
            4 => self.read_post_meta_or_ctrl_char(*p).map_err(Err)?,
            5 => self.strings.escape = Some(self.encode_escape(self.escape_ord() & 0x9f)),
            6 => self.strings.escape = Some(self.encode_escape(self.escape_ord() | 0x80)),
            7 | 9 => {
                let digits = self.s_tok(*p - 2, *p);
                let ord = u32::from_str_radix(&digits, 16).unwrap_or(0);
                self.strings.escape = Some(self.encode_escape(ord));
            }
            8 => self.strings.escape = Some((vec![0x7f], self.encoding)),
            10 => {
                let digits = self.s_tok(self.strings.escape_s, *p);
                let ord = u32::from_str_radix(&digits, 8).unwrap_or(0) % 0x100;
                self.strings.escape = Some(self.encode_escape(ord));
            }
            11 => {
                let digits = self.s_tok(self.strings.escape_s + 1, *p);
                let ord = u32::from_str_radix(&digits, 16).unwrap_or(0);
                self.strings.escape = Some(self.encode_escape(ord));
            }
            12 => {
                let s = self.strings.escape_s - 1;
                return fatal(self, "invalid hex escape", (s, *p + 2));
            }
            13 => {
                let digits = self.s_tok(self.strings.escape_s + 1, *p);
                let ord = u32::from_str_radix(&digits, 16).unwrap_or(0);
                let bytes = char::from_u32(ord)
                    .map(String::from)
                    .unwrap_or_default()
                    .into_bytes();
                self.strings.escape = Some((bytes, LiteralEncoding::Text));
            }
            14 | 15 => self.check_invalid_escapes(*p).map_err(Err)?,
            16 => return fatal(self, "unterminated Unicode escape", (*p - 1, *p)),
            17 => return fatal(self, "escape sequence meets end of file", (*p - 1, *p)),
            18 => {
                self.strings.escape_s = *p;
                self.strings.escape = None;
            }
            19 => *interp_var_kind = "gvar",
            20 => *interp_var_kind = "cvar",
            21 => *interp_var_kind = "ivar",
            22 => self.strings.escape = None,
            23 => self.strings.ts = NIL,
            24 => self.strings.ts = *p,
            25 => self.strings.te = *p + 1,
            26 | 36 | 55 | 64 => {
                self.strings.te = *p + 1;
                return self.run_body(Body::InterpCode, p, pe, lookahead, interp_var_kind);
            }
            27 | 37 | 46 | 50 | 52 | 56 | 62 | 65 | 72 => {
                self.strings.te = *p + 1;
                return self.run_body(Body::StringEol, p, pe, lookahead, interp_var_kind);
            }
            28 | 38 | 47 | 53 | 57 | 63 | 66 | 73 => {
                self.strings.te = *p + 1;
                return self.run_body(Body::String, p, pe, lookahead, interp_var_kind);
            }
            29 | 39 | 58 | 67 => {
                self.strings.te = *p;
                *p -= 1;
                return self.run_body(Body::InterpDigitVar, p, pe, lookahead, interp_var_kind);
            }
            30 | 40 | 59 | 68 => {
                self.strings.te = *p;
                *p -= 1;
                return self.run_body(Body::InterpVar, p, pe, lookahead, interp_var_kind);
            }
            31 | 41 => {
                self.strings.te = *p;
                *p -= 1;
                return self.run_body(Body::StringEscaped, p, pe, lookahead, interp_var_kind);
            }
            32 | 48 | 69 | 74 => {
                self.strings.te = *p;
                *p -= 1;
                return self.run_body(Body::StringSpace, p, pe, lookahead, interp_var_kind);
            }
            33 | 42 | 49 | 54 | 60 | 70 => {
                self.strings.te = *p;
                *p -= 1;
                return self.run_body(Body::String, p, pe, lookahead, interp_var_kind);
            }
            34 | 43 => {
                *p = self.strings.te - 1;
                return self.run_body(Body::StringEscaped, p, pe, lookahead, interp_var_kind);
            }
            35 | 44 | 61 | 71 => {
                *p = self.strings.te - 1;
                return self.run_body(Body::String, p, pe, lookahead, interp_var_kind);
            }
            45 | 51 => {
                self.strings.te = *p + 1;
                return self.run_body(Body::StringEscaped, p, pe, lookahead, interp_var_kind);
            }
            75 => {
                self.strings.te = *p + 1;
                let (ts, te) = self.s_range();
                let value = self.s_tok(ts, te - 1);
                self.s_emit(
                    Tok::tREGEXP_OPT,
                    TokenValue::from(value.as_str()),
                    ts,
                    te - 1,
                );
                *p -= 1;
                self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
                *p += 1;
                return Err(Ok(Break));
            }
            76 => {
                self.strings.te = *p;
                *p -= 1;
                let (ts, te) = self.s_range();
                let tok = self.s_tok(ts, te);
                let unknown: String = tok.chars().filter(|c| !"imxouesn".contains(*c)).collect();
                if !unknown.is_empty() {
                    self.s_diagnostic(
                        Level::Error,
                        &format!("unknown regexp options: {unknown}"),
                        (ts, te),
                    );
                }
                self.s_emit(Tok::tREGEXP_OPT, TokenValue::from(tok.as_str()), ts, te);
                self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
                *p += 1;
                return Err(Ok(Break));
            }
            77 => {
                // `?` の後の空白: warning だけ出して三項演算子として読み直す。
                self.strings.te = *p + 1;
                *p = self.strings.ts - 1;
                self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
                *p += 1;
                return Err(Ok(Break));
            }
            78 => {
                self.strings.te = *p;
                *p -= 1;
                self.emit_character_constant();
                self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
                *p += 1;
                return Err(Ok(Break));
            }
            79 => {
                self.strings.te = *p;
                *p -= 1;
                *p = self.strings.ts - 1;
                self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
                *p += 1;
                return Err(Ok(Break));
            }
            80 => {
                *p = self.strings.te - 1;
                self.emit_character_constant();
                self.strings.root_lexer_state = main_tables::EN_EXPR_END as i64;
                *p += 1;
                return Err(Ok(Break));
            }
            81 => {
                // `unknown` 状態 (`raise 'bug'`)。push_literal を経ずに来ることは無い。
                self.strings.te = *p + 1;
                unreachable!("LexerStrings の unknown 状態に来た");
            }
            _ => unreachable!("LexerStrings に無い動作 {action}"),
        }
        Ok(())
    }
}

/// `_lex_actions[acts]` から始まる動作の並び。
fn action_list(acts: usize) -> &'static [u16] {
    let count = usize::from(ACTIONS[acts]);
    &ACTIONS[acts + 1..acts + 1 + count]
}

fn next_state_for_literal(literal: &Literal) -> i64 {
    (match (
        literal.words(),
        literal.backslash_delimited(),
        literal.interpolate(),
    ) {
        (true, true, true) => EN_INTERP_BACKSLASH_DELIMITED_WORDS,
        (true, true, false) => EN_PLAIN_BACKSLASH_DELIMITED_WORDS,
        (true, false, true) => EN_INTERP_WORDS,
        (true, false, false) => EN_PLAIN_WORDS,
        (false, true, true) => EN_INTERP_BACKSLASH_DELIMITED,
        (false, true, false) => EN_PLAIN_BACKSLASH_DELIMITED,
        (false, false, true) => EN_INTERP_STRING,
        (false, false, false) => EN_PLAIN_STRING,
    }) as i64
}

#[cfg(test)]
mod tests {
    /// RuboCop::AST::Builder は不正 UTF-8 の literal 値も受理する (parser の既定 builder と異なる)。
    /// CLI で記録した陰性対照を、改行・版・符号化宣言を変えても拒まない。
    #[test]
    fn byte_escapes_keep_values_without_adding_default_builder_encoding_errors() {
        use crate::legacy_parser::{Version, diagnostics};
        let cases = [
            (r#""\x80""#, 0..6),
            (r"?\x80", 0..5),
            (r#""\M-a""#, 0..6),
            (r#""\C-\M-a""#, 0..9),
            (r#""\M-\C-a""#, 0..9),
            (r#""\c\M-a""#, 0..8),
            (r#""\x80#{a}""#, 1..5),
            (r"%W[\x80]", 3..7),
        ];
        for version in [
            Version::Ruby27,
            Version::Ruby30,
            Version::Ruby31,
            Version::Ruby32,
        ] {
            for newline in ["\n", "\r\n"] {
                for (body, _range) in &cases {
                    let source = format!("{body}{newline}");
                    let found = diagnostics(&source, version);
                    assert!(found.is_empty(), "{version:?} {source:?} {found:?}");
                    for label in ["ASCII-8BIT", "BINARY"] {
                        let binary = format!("# encoding: {label}{newline}{body}{newline}");
                        assert!(
                            diagnostics(&binary, version).is_empty(),
                            "{version:?} {binary:?}"
                        );
                    }
                }
                // バイト列全体で有効な UTF-8、Unicode escape、単一引用の未復号値を拒まない。
                // Source::Buffer は US-ASCII の原本も UTF-8 に復号してから lexer へ渡す。
                for prefix in ["", "# encoding: US-ASCII\n", "# encoding: ASCII-8BIT\n"] {
                    for body in [r#""\xC3\xA9""#, r#""\u00e9""#, r#"'\x80'"#, r"%q(\200)"] {
                        let source = format!("{prefix}{body}\n").replace('\n', newline);
                        assert!(
                            diagnostics(&source, version).is_empty(),
                            "{version:?} {source:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_hand_ported_actions_follow_the_generated_lexer_they_were_ported_from() {
        assert_eq!(
            crate::legacy_parser::tables::lexer_strings::SOURCE_SHA256,
            "6f0c912b71462338e8502a80e76a02e04cda5d80e97219b984ad7e89bef023e4"
        );
    }
}
