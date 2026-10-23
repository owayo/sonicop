//! parser gem 3.3.12.0 の字句解析器 (`Parser::Lexer`、`lexer.rl`) を移したもの。
//!
//! Ragel の遷移表と、遷移に付いた動作の本体 (`exec_main`) は gem の生成物から機械的に写した
//! 生成物 (`tables/lexer.rs`・`tables/lexer_exec.rs`) で、ここにはその動作が呼ぶ補助メソッドと
//! 字句解析器の状態を置く。補助メソッドは `lexer.rl` の同名のメソッドを 1 つずつ写している。
//!
//! 位置は parser gem と同じく `source_pts` (コードポイントの配列) の添字で持つ。Ragel の動作は
//! 位置を一時的に負にする (`p = @ts - 1`) ので i64 で扱う。

mod literal;
mod strings;

use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use super::parser::Fatal;
use super::tables::lexer::*;
use super::tables::tokens::Tok;
use super::value::{Context, Token, TokenValue};
use super::{Diagnostic, Level};

pub(crate) use strings::Strings;

/// Ruby の `nil` の代わり (位置が無いこと)。
pub(crate) const NIL: i64 = i64::MIN;

// Ragel の `_goto_level` の段階。
pub(crate) const L_RESUME: i32 = 10;
pub(crate) const L_EOF_TRANS: i32 = 15;
pub(crate) const L_AGAIN: i32 = 20;
pub(crate) const L_TEST_EOF: i32 = 30;
pub(crate) const L_OUT: i32 = 40;

/// 文法動作が `@lexer.state = :expr_...` で指定する状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum LexState {
    ExprBeg,
    ExprEnd,
    ExprEndarg,
    ExprFname,
    ExprValue,
}

/// `Parser::Lexer::StackState`。Ruby の整数のビット列をそのまま写すと桁あふれするので、
/// 下位ビットを末尾に置いた配列で持つ (配列より上は 0)。
#[derive(Clone, Debug, Default)]
pub(crate) struct StackState {
    bits: Vec<bool>,
}

impl StackState {
    pub(crate) fn push(&mut self, bit: bool) {
        self.bits.push(bit);
    }

    pub(crate) fn pop(&mut self) -> bool {
        self.bits.pop().unwrap_or(false)
    }

    /// `@stack = ((@stack >> 1) | (@stack & 1))`。
    pub(crate) fn lexpop(&mut self) -> bool {
        let low = self.pop();
        match self.bits.last_mut() {
            Some(top) => *top |= low,
            None if low => self.bits.push(true),
            None => {}
        }
        self.is_active()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.bits.last().copied().unwrap_or(false)
    }
}

/// `Parser::StaticEnvironment`。
#[derive(Clone, Debug, Default)]
pub(crate) struct StaticEnv {
    variables: HashSet<Rc<str>>,
    stack: Vec<HashSet<Rc<str>>>,
}

const FORWARD_ARGS: &str = "FORWARD_ARGS";
const ANONYMOUS_RESTARG_IN_CURRENT_SCOPE: &str = "ANONYMOUS_RESTARG_IN_CURRENT_SCOPE";
const ANONYMOUS_RESTARG_INHERITED: &str = "ANONYMOUS_RESTARG_INHERITED";
const ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE: &str = "ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE";
const ANONYMOUS_KWRESTARG_INHERITED: &str = "ANONYMOUS_KWRESTARG_INHERITED";
const ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE: &str = "ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE";
const ANONYMOUS_BLOCKARG_INHERITED: &str = "ANONYMOUS_BLOCKARG_INHERITED";

impl StaticEnv {
    pub(crate) fn extend_static(&mut self) {
        let current = std::mem::take(&mut self.variables);
        self.stack.push(current);
    }

    pub(crate) fn extend_dynamic(&mut self) {
        let mut next = self.variables.clone();
        for (current, inherited) in [
            (
                ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE,
                ANONYMOUS_BLOCKARG_INHERITED,
            ),
            (
                ANONYMOUS_RESTARG_IN_CURRENT_SCOPE,
                ANONYMOUS_RESTARG_INHERITED,
            ),
            (
                ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE,
                ANONYMOUS_KWRESTARG_INHERITED,
            ),
        ] {
            if next.remove(current) {
                next.insert(Rc::from(inherited));
            }
        }
        let current = std::mem::replace(&mut self.variables, next);
        self.stack.push(current);
    }

    pub(crate) fn unextend(&mut self) {
        // `@variables = @stack.pop` は空のスタックで nil になる。文法がそれを起こさない限り到達しない。
        self.variables = self.stack.pop().unwrap_or_default();
    }

    pub(crate) fn declare(&mut self, name: &str) {
        self.variables.insert(Rc::from(name));
    }

    pub(crate) fn is_declared(&self, name: &str) -> bool {
        self.variables.contains(name)
    }

    pub(crate) fn declare_forward_args(&mut self) {
        self.declare(FORWARD_ARGS);
    }

    pub(crate) fn declared_forward_args(&self) -> bool {
        self.is_declared(FORWARD_ARGS)
    }

    pub(crate) fn declare_anonymous_blockarg(&mut self) {
        self.declare(ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE);
    }

    pub(crate) fn declared_anonymous_blockarg(&self) -> bool {
        self.is_declared(ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE)
            || self.is_declared(ANONYMOUS_BLOCKARG_INHERITED)
    }

    pub(crate) fn declare_anonymous_restarg(&mut self) {
        self.declare(ANONYMOUS_RESTARG_IN_CURRENT_SCOPE);
    }

    pub(crate) fn declared_anonymous_restarg(&self) -> bool {
        self.is_declared(ANONYMOUS_RESTARG_IN_CURRENT_SCOPE)
            || self.is_declared(ANONYMOUS_RESTARG_INHERITED)
    }

    pub(crate) fn declare_anonymous_kwrestarg(&mut self) {
        self.declare(ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE);
    }

    pub(crate) fn declared_anonymous_kwrestarg(&self) -> bool {
        self.is_declared(ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE)
            || self.is_declared(ANONYMOUS_KWRESTARG_INHERITED)
    }

    /// `declared_anonymous_blockarg_in_current_scpe?` (gem 側の綴りのまま)。
    pub(crate) fn declared_anonymous_blockarg_in_current_scope(&self) -> bool {
        self.is_declared(ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE)
    }

    pub(crate) fn declared_anonymous_restarg_in_current_scope(&self) -> bool {
        self.is_declared(ANONYMOUS_RESTARG_IN_CURRENT_SCOPE)
    }

    pub(crate) fn declared_anonymous_kwrestarg_in_current_scope(&self) -> bool {
        self.is_declared(ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE)
    }

    pub(crate) fn parent_has_anonymous_blockarg(&self) -> bool {
        self.stack
            .iter()
            .any(|variables| variables.contains(ANONYMOUS_BLOCKARG_IN_CURRENT_SCOPE))
    }

    pub(crate) fn parent_has_anonymous_restarg(&self) -> bool {
        self.stack
            .iter()
            .any(|variables| variables.contains(ANONYMOUS_RESTARG_IN_CURRENT_SCOPE))
    }

    pub(crate) fn parent_has_anonymous_kwrestarg(&self) -> bool {
        self.stack
            .iter()
            .any(|variables| variables.contains(ANONYMOUS_KWRESTARG_IN_CURRENT_SCOPE))
    }
}

/// `@num_xfrm` に入る lambda (`int_suffix` などが選ぶ)。
#[derive(Clone, Copy, Debug)]
pub(crate) enum NumXfrm {
    Integer,
    Rational,
    Imaginary,
    ImaginaryRational,
    IntegerRe,
    IntegerIf,
    IntegerRescue,
    Float,
    ImaginaryFloat,
    FloatIf,
    FloatRescue,
}

/// `emit_table` が引く表。
#[derive(Clone, Copy, Debug)]
pub(crate) enum Table {
    Punctuation,
    PunctuationBegin,
    Keywords,
    KeywordsBegin,
}

pub(crate) struct Lexer {
    pub(crate) version: i64,
    pub(crate) encoding: crate::engine::LiteralEncoding,
    /// `@source_pts`。
    pub(crate) src: Vec<u32>,
    pub(crate) static_env: StaticEnv,
    pub(crate) context: Context,
    pub(crate) cond: StackState,
    pub(crate) cmdarg: StackState,
    pub(crate) cs: i64,
    p: i64,
    pub(crate) ts: i64,
    pub(crate) te: i64,
    pub(crate) act: i64,
    stack: Vec<i64>,
    pub(crate) top: i64,
    pub(crate) token_queue: VecDeque<Token>,
    pub(crate) eq_begin_s: i64,
    pub(crate) sharp_s: i64,
    pub(crate) newline_s: i64,
    pub(crate) num_base: i64,
    pub(crate) num_digits_s: i64,
    pub(crate) num_suffix_s: i64,
    pub(crate) num_xfrm: NumXfrm,
    pub(crate) paren_nest: i64,
    pub(crate) lambda_stack: Vec<i64>,
    pub(crate) command_start: bool,
    pub(crate) cs_before_block_comment: i64,
    pub(crate) strings: Strings,
    /// 字句解析器の診断。構文解析器が `advance` のたびに引き取る。
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// 出したトークンの記録 (`@tokens`)。比較用の道具だけが使う。
    pub(crate) recorded: Option<Vec<Token>>,
}

impl Lexer {
    /// `Lexer.new(version)` と `source_buffer=`。`source` は CRLF を LF にした本文のコードポイント。
    pub(crate) fn new(version: i64, source: Vec<u32>) -> Self {
        let p = i64::from(source.first() == Some(&0xfeff));
        let prefix_end = source
            .iter()
            .enumerate()
            .filter(|(_, point)| **point == u32::from('\n'))
            .nth(1)
            .map_or(source.len(), |(index, _)| index + 1);
        let prefix: String = source[..prefix_end]
            .iter()
            .copied()
            .filter_map(char::from_u32)
            .collect();
        // Source::Buffer#reencode_string は BINARY 以外の宣言を UTF-8 に復号する。
        let encoding = if crate::engine::declared_literal_encoding(&prefix)
            == crate::engine::LiteralEncoding::Binary
        {
            crate::engine::LiteralEncoding::Binary
        } else {
            crate::engine::LiteralEncoding::Text
        };
        Self {
            version,
            encoding,
            src: source,
            static_env: StaticEnv::default(),
            context: Context::default(),
            cond: StackState::default(),
            cmdarg: StackState::default(),
            cs: EN_LINE_BEGIN as i64,
            p,
            ts: NIL,
            te: NIL,
            act: 0,
            stack: Vec::new(),
            top: 0,
            token_queue: VecDeque::new(),
            eq_begin_s: NIL,
            sharp_s: NIL,
            newline_s: NIL,
            num_base: NIL,
            num_digits_s: NIL,
            num_suffix_s: NIL,
            num_xfrm: NumXfrm::Integer,
            paren_nest: 0,
            lambda_stack: Vec::new(),
            command_start: true,
            cs_before_block_comment: EN_LINE_BEGIN as i64,
            strings: Strings::new(),
            diagnostics: Vec::new(),
            recorded: None,
        }
    }

    /// `@lexer.state = ...`。
    pub(crate) fn set_state(&mut self, state: LexState) {
        self.cs = match state {
            LexState::ExprBeg => EN_EXPR_BEG,
            LexState::ExprEnd => EN_EXPR_END,
            LexState::ExprEndarg => EN_EXPR_ENDARG,
            LexState::ExprFname => EN_EXPR_FNAME,
            LexState::ExprValue => EN_EXPR_VALUE,
        } as i64;
    }

    /// `@lexer.dedent_level` (読むと消える)。
    pub(crate) fn take_dedent_level(&mut self) -> Option<i64> {
        self.strings.dedent_level.take()
    }

    /// `advance`: 次のトークン。
    pub(crate) fn advance(&mut self) -> Result<Token, Fatal> {
        if let Some(token) = self.token_queue.pop_front() {
            return Ok(token);
        }
        let pe = self.src.len() as i64 + 2;
        let p = self.p;
        let cmd_state = self.command_start;
        self.command_start = false;
        let p = self.exec_main(p, pe, pe, cmd_state)?;
        self.p = p;
        if let Some(token) = self.token_queue.pop_front() {
            Ok(token)
        } else if self.cs == ERROR as i64 {
            Ok(Token {
                tok: Tok::EOF,
                value: TokenValue::Str(Rc::from("$error")),
                range: to_index(p - 1)..to_index(p),
            })
        } else {
            let eof = self.src.len();
            Ok(Token {
                tok: Tok::EOF,
                value: TokenValue::Str(Rc::from("$eof")),
                range: eof..eof,
            })
        }
    }

    /// Ragel の遷移の選び方 (`_keys`・`_inds`・`_slen`・`_wide`)。
    pub(crate) fn transition(&self, p: i64) -> usize {
        let cs = self.cs as usize;
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

    /// `@source_pts[p] || 0`。Ruby の負の添字は末尾から数える。
    pub(crate) fn point(&self, p: i64) -> u32 {
        let len = self.src.len() as i64;
        let index = if p < 0 { len + p } else { p };
        if (0..len).contains(&index) {
            self.src[index as usize]
        } else {
            0
        }
    }

    /// `tok(s, e)` = `@source_buffer.slice(s, e - s)` (`String#[start, length]`)。
    pub(crate) fn tok(&self, s: i64, e: i64) -> String {
        slice_points(&self.src, s, e - s)
    }

    pub(crate) fn version_in(&self, versions: &[i64]) -> bool {
        versions.contains(&self.version)
    }

    pub(crate) fn emit(&mut self, tok: Tok, value: TokenValue, s: i64, e: i64) {
        let token = Token {
            tok,
            value,
            range: to_index(s)..to_index(e),
        };
        if let Some(recorded) = self.recorded.as_mut() {
            recorded.push(token.clone());
        }
        self.token_queue.push_back(token);
    }

    pub(crate) fn emit_table(&mut self, table: Table, s: i64, e: i64) {
        let value = self.tok(s, e);
        let tok = lookup(table, &value).expect("emit_table は表にある字句にだけ呼ばれる");
        self.emit(tok, TokenValue::from(value.as_str()), s, e);
    }

    pub(crate) fn emit_do(&mut self, do_block: bool) {
        let tok = if self.cond.is_active() {
            Tok::kDO_COND
        } else if self.cmdarg.is_active() || do_block {
            Tok::kDO_BLOCK
        } else {
            Tok::kDO
        };
        self.emit(tok, TokenValue::from("do"), self.ts, self.te);
    }

    pub(crate) fn arg_or_cmdarg(&self, cmd_state: bool) -> i64 {
        if cmd_state {
            EN_EXPR_CMDARG as i64
        } else {
            EN_EXPR_ARG as i64
        }
    }

    /// コメントは構文解析に関わらない (`@comments` と `@tokens` への記録だけ)。
    pub(crate) fn emit_comment_from_range(&mut self, _p: i64, _pe: i64) {}

    pub(crate) fn emit_comment(&mut self, _s: i64, _e: i64) {}

    pub(crate) fn diagnostic(&mut self, level: Level, message: String, (s, e): (i64, i64)) {
        self.diagnostics.push(Diagnostic {
            level,
            message,
            range: to_index(s)..to_index(e),
        });
    }

    /// 名前で選ぶ診断 (`diag_msg`)。
    pub(crate) fn message_for(&self, reason: &str, arguments: &[(&str, String)]) -> String {
        let name = arguments
            .iter()
            .find(|(key, _)| *key == "name")
            .map(|(_, value)| value.as_str())
            .unwrap_or_default();
        match reason {
            "ivar_name" => format!("`{name}' is not allowed as an instance variable name"),
            "cvar_name" => format!("`{name}' is not allowed as a class variable name"),
            _ => unreachable!("名前で選ぶ診断は ivar_name と cvar_name だけ"),
        }
    }

    pub(crate) fn numeric_literal_int(&mut self) -> String {
        let digits = self.tok(self.num_digits_s, self.num_suffix_s);
        if digits.ends_with('_') {
            self.diagnostic(
                Level::Error,
                "trailing `_' in number".to_owned(),
                (self.te - 1, self.te),
            );
        } else if digits.is_empty() && self.num_base == 8 && self.version_in(&[18]) {
            return "0".to_owned();
        } else if digits.is_empty() {
            self.diagnostic(
                Level::Error,
                "numeric literal without digits".to_owned(),
                (self.ts, self.te),
            );
        } else if self.num_base == 8
            && let Some(invalid) = digits.chars().position(|c| c == '8' || c == '9')
        {
            let invalid_s = self.num_digits_s + invalid as i64;
            self.diagnostic(
                Level::Error,
                "invalid octal digit".to_owned(),
                (invalid_s, invalid_s + 1),
            );
        }
        digits
    }

    pub(crate) fn num_xfrm_call(&mut self, chars: String, p: i64) -> i64 {
        let value = TokenValue::from(chars.as_str());
        let (tok, cut) = match self.num_xfrm {
            NumXfrm::Integer => (Tok::tINTEGER, 0),
            NumXfrm::Rational => (Tok::tRATIONAL, 0),
            NumXfrm::Imaginary | NumXfrm::ImaginaryRational | NumXfrm::ImaginaryFloat => {
                (Tok::tIMAGINARY, 0)
            }
            NumXfrm::IntegerRe | NumXfrm::IntegerIf => (Tok::tINTEGER, 2),
            NumXfrm::IntegerRescue => (Tok::tINTEGER, 6),
            NumXfrm::Float => (Tok::tFLOAT, 0),
            NumXfrm::FloatIf => (Tok::tFLOAT, 2),
            NumXfrm::FloatRescue => (Tok::tFLOAT, 6),
        };
        self.emit(tok, value, self.ts, self.te - cut);
        p - cut
    }

    pub(crate) fn stack_pop(&mut self) -> i64 {
        self.top -= 1;
        self.stack.get(self.top as usize).copied().unwrap_or(NIL)
    }

    pub(crate) fn stack_set(&mut self, index: i64, value: i64) {
        let index = index as usize;
        if self.stack.len() <= index {
            self.stack.resize(index + 1, NIL);
        }
        self.stack[index] = value;
    }

    pub(crate) fn e_lbrace(&mut self) {
        self.cond.push(false);
        self.cmdarg.push(false);
        if let Some(literal) = self.strings.literal_stack.last_mut() {
            literal.start_interp_brace();
        }
    }

    pub(crate) fn emit_singleton_class(&mut self) {
        self.emit(Tok::kCLASS, TokenValue::from("class"), self.ts, self.ts + 5);
        self.emit(Tok::tLSHFT, TokenValue::from("<<"), self.te - 2, self.te);
    }

    pub(crate) fn emit_rbrace_rparen_rbrack(&mut self) {
        self.emit_table(Table::Punctuation, self.ts, self.te);
        if self.version < 24 {
            self.cond.lexpop();
            self.cmdarg.lexpop();
        } else {
            self.cond.pop();
            self.cmdarg.pop();
        }
    }

    pub(crate) fn emit_global_var(&mut self) {
        self.emit_global_var_at(self.ts, self.te);
    }

    /// `emit_global_var(ts, te)`。2 つ目の判定は既定の `tok` (この字句解析器の `@ts`・`@te`) を
    /// 見る。文字列の補間から呼ばれたときは別の範囲になるが、parser gem のとおりにする。
    pub(crate) fn emit_global_var_at(&mut self, ts: i64, te: i64) {
        let text = self.tok(ts, te);
        if any_line(&text, |line| {
            let digits = line.strip_prefix('$').unwrap_or("");
            digits.starts_with(|c: char| ('1'..='9').contains(&c))
                && digits.chars().all(|c| c.is_ascii_digit())
        }) {
            let value = self.tok(ts + 1, te);
            self.emit(Tok::tNTH_REF, TokenValue::from(value.as_str()), ts, te);
        } else if any_line(&self.tok(self.ts, self.te), |line| {
            matches!(line, "$&" | "$`" | "$'" | "$+")
        }) {
            self.emit(Tok::tBACK_REF, TokenValue::from(text.as_str()), ts, te);
        } else {
            self.emit(Tok::tGVAR, TokenValue::from(text.as_str()), ts, te);
        }
    }

    pub(crate) fn emit_class_var(&mut self) {
        self.emit_class_var_at(self.ts, self.te);
    }

    pub(crate) fn emit_class_var_at(&mut self, ts: i64, te: i64) {
        let text = self.tok(ts, te);
        if any_line(&text, |line| {
            line.strip_prefix("@@")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        }) {
            let message = format!("`{text}' is not allowed as a class variable name");
            self.diagnostic(Level::Error, message, (self.ts, self.te));
        }
        self.emit(Tok::tCVAR, TokenValue::from(text.as_str()), ts, te);
    }

    pub(crate) fn emit_instance_var(&mut self) {
        self.emit_instance_var_at(self.ts, self.te);
    }

    pub(crate) fn emit_instance_var_at(&mut self, ts: i64, te: i64) {
        let text = self.tok(ts, te);
        if any_line(&text, |line| {
            line.strip_prefix('@')
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        }) {
            let message = format!("`{text}' is not allowed as an instance variable name");
            self.diagnostic(Level::Error, message, (self.ts, self.te));
        }
        self.emit(Tok::tIVAR, TokenValue::from(text.as_str()), ts, te);
    }

    pub(crate) fn emit_colon_with_digits(&mut self, p: i64, tm: i64, diag_msg: &str) -> i64 {
        if self.version >= 27 {
            let name = self.tok(tm, self.te);
            let message = self.message_for(diag_msg, &[("name", name)]);
            self.diagnostic(Level::Error, message, (tm, self.te));
            p
        } else {
            let value = self.tok(self.ts, self.ts + 1);
            self.emit(
                Tok::tCOLON,
                TokenValue::from(value.as_str()),
                self.ts,
                self.ts + 1,
            );
            self.ts
        }
    }

    /// `on_newline(p)` = `@strings.on_newline(p)`。
    pub(crate) fn on_newline(&mut self, p: i64) -> i64 {
        match self.strings.herebody_s.take() {
            Some(herebody_s) => herebody_s,
            None => p,
        }
    }
}

fn to_index(position: i64) -> usize {
    usize::try_from(position).unwrap_or(0)
}

/// Ruby の `^...$` は行ごとに照合する。
fn any_line(text: &str, predicate: impl Fn(&str) -> bool) -> bool {
    text.split('\n').any(predicate)
}

/// `String#[start, length]` (コードポイント単位)。範囲外は空文字列。
pub(crate) fn slice_points(points: &[u32], start: i64, length: i64) -> String {
    let len = points.len() as i64;
    let start = if start < 0 { len + start } else { start };
    if start < 0 || start > len || length < 0 {
        return String::new();
    }
    let end = (start + length).min(len);
    points[start as usize..end as usize]
        .iter()
        .map(|&point| char::from_u32(point).unwrap_or('\u{fffd}'))
        .collect()
}

/// `string[a..b]` (負の端は末尾から)。
pub(crate) fn str_slice(text: &str, from: i64, to: i64) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len() as i64;
    let from = if from < 0 { len + from } else { from };
    let to = if to < 0 { len + to } else { to };
    if from < 0 || from > len || to < from - 1 {
        return String::new();
    }
    let end = (to + 1).min(len);
    chars[from as usize..end.max(from) as usize]
        .iter()
        .collect()
}

/// `string[i]` (負の添字は末尾から)。
pub(crate) fn str_char_at(text: &str, index: i64) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len() as i64;
    let index = if index < 0 { len + index } else { index };
    if (0..len).contains(&index) {
        chars[index as usize].to_string()
    } else {
        String::new()
    }
}

/// `String#inspect` (二重引用符を含む)。`unexpected` の診断が `[1..-2]` で引用符を外して使う。
pub(crate) fn ruby_inspect(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            '\u{8}' => out.push_str("\\b"),
            '\u{7}' => out.push_str("\\a"),
            '\u{1b}' => out.push_str("\\e"),
            '#' => out.push('#'),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `tok(@ts, heredoc_e) =~ /^<<(-?)(~?)(["'`]?)(.*)\3$/m` の `$1`〜`$4`。
pub(crate) fn heredoc_captures(text: &str) -> [String; 5] {
    let mut captures: [String; 5] = Default::default();
    let Some(rest) = text.strip_prefix("<<") else {
        return captures;
    };
    let mut rest = rest;
    if let Some(after) = rest.strip_prefix('-') {
        captures[1] = "-".to_owned();
        rest = after;
    }
    if let Some(after) = rest.strip_prefix('~') {
        captures[2] = "~".to_owned();
        rest = after;
    }
    let quote = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\'' | '`'));
    match quote {
        Some(quote) if rest.len() > 1 && rest.ends_with(quote) => {
            captures[3] = quote.to_string();
            captures[4] = rest[1..rest.len() - 1].to_owned();
        }
        _ => {
            captures[4] = rest.to_owned();
        }
    }
    captures[0] = text.to_owned();
    captures
}

/// `/\A_[1-9]\z/`。
pub(crate) fn is_numparam_name(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 2 && bytes[0] == b'_' && (b'1'..=b'9').contains(&bytes[1])
}

impl From<&str> for TokenValue {
    fn from(text: &str) -> Self {
        TokenValue::Str(Rc::from(text))
    }
}

/// `PUNCTUATION` / `PUNCTUATION_BEGIN` / `KEYWORDS` / `KEYWORDS_BEGIN`。
fn lookup(table: Table, text: &str) -> Option<Tok> {
    let keyword = |text: &str| -> Option<Tok> {
        Some(match text {
            "class" => Tok::kCLASS,
            "module" => Tok::kMODULE,
            "def" => Tok::kDEF,
            "undef" => Tok::kUNDEF,
            "begin" => Tok::kBEGIN,
            "end" => Tok::kEND,
            "then" => Tok::kTHEN,
            "elsif" => Tok::kELSIF,
            "else" => Tok::kELSE,
            "ensure" => Tok::kENSURE,
            "case" => Tok::kCASE,
            "when" => Tok::kWHEN,
            "for" => Tok::kFOR,
            "break" => Tok::kBREAK,
            "next" => Tok::kNEXT,
            "redo" => Tok::kREDO,
            "retry" => Tok::kRETRY,
            "in" => Tok::kIN,
            "do" => Tok::kDO,
            "return" => Tok::kRETURN,
            "yield" => Tok::kYIELD,
            "super" => Tok::kSUPER,
            "self" => Tok::kSELF,
            "nil" => Tok::kNIL,
            "true" => Tok::kTRUE,
            "false" => Tok::kFALSE,
            "and" => Tok::kAND,
            "or" => Tok::kOR,
            "not" => Tok::kNOT,
            "alias" => Tok::kALIAS,
            "__FILE__" => Tok::k__FILE__,
            "__LINE__" => Tok::k__LINE__,
            "__ENCODING__" => Tok::k__ENCODING__,
            "defined?" => Tok::kDEFINED,
            "BEGIN" => Tok::klBEGIN,
            "END" => Tok::klEND,
            _ => return None,
        })
    };
    match table {
        Table::Keywords => Some(match text {
            "if" => Tok::kIF_MOD,
            "unless" => Tok::kUNLESS_MOD,
            "while" => Tok::kWHILE_MOD,
            "until" => Tok::kUNTIL_MOD,
            "rescue" => Tok::kRESCUE_MOD,
            _ => return keyword(text),
        }),
        Table::KeywordsBegin => Some(match text {
            "if" => Tok::kIF,
            "unless" => Tok::kUNLESS,
            "while" => Tok::kWHILE,
            "until" => Tok::kUNTIL,
            "rescue" => Tok::kRESCUE,
            _ => return keyword(text),
        }),
        Table::PunctuationBegin => Some(match text {
            "&" => Tok::tAMPER,
            "*" => Tok::tSTAR,
            "**" => Tok::tDSTAR,
            "+" => Tok::tUPLUS,
            "-" => Tok::tUMINUS,
            "::" => Tok::tCOLON3,
            "(" => Tok::tLPAREN,
            "{" => Tok::tLBRACE,
            "[" => Tok::tLBRACK,
            _ => return None,
        }),
        Table::Punctuation => Some(match text {
            "=" => Tok::tEQL,
            "&" => Tok::tAMPER2,
            "|" => Tok::tPIPE,
            "!" | "!@" => Tok::tBANG,
            "^" => Tok::tCARET,
            "+" => Tok::tPLUS,
            "-" => Tok::tMINUS,
            "*" => Tok::tSTAR2,
            "/" => Tok::tDIVIDE,
            "%" => Tok::tPERCENT,
            "~" | "~@" => Tok::tTILDE,
            "," => Tok::tCOMMA,
            ";" => Tok::tSEMI,
            "." => Tok::tDOT,
            ".." => Tok::tDOT2,
            "..." => Tok::tDOT3,
            "[" => Tok::tLBRACK2,
            "]" => Tok::tRBRACK,
            "(" => Tok::tLPAREN2,
            ")" => Tok::tRPAREN,
            "?" => Tok::tEH,
            ":" => Tok::tCOLON,
            "&&" => Tok::tANDOP,
            "||" => Tok::tOROP,
            "-@" => Tok::tUMINUS,
            "+@" => Tok::tUPLUS,
            "**" => Tok::tPOW,
            "->" => Tok::tLAMBDA,
            "=~" => Tok::tMATCH,
            "!~" => Tok::tNMATCH,
            "==" => Tok::tEQ,
            "!=" => Tok::tNEQ,
            ">" => Tok::tGT,
            ">>" => Tok::tRSHFT,
            ">=" => Tok::tGEQ,
            "<" => Tok::tLT,
            "<<" => Tok::tLSHFT,
            "<=" => Tok::tLEQ,
            "=>" => Tok::tASSOC,
            "::" => Tok::tCOLON2,
            "===" => Tok::tEQQ,
            "<=>" => Tok::tCMP,
            "[]" => Tok::tAREF,
            "[]=" => Tok::tASET,
            "{" => Tok::tLCURLY,
            "}" => Tok::tRCURLY,
            "`" => Tok::tBACK_REF2,
            "&." => Tok::tANDDOT,
            _ => return None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hand_ported_helpers_follow_the_generated_lexer_they_were_ported_from() {
        // 動作の番号と補助メソッドは、この版の生成物を写したもの。gem を更新して表を作り直したら
        // ここも見直す。
        assert_eq!(
            SOURCE_SHA256,
            "0ec464f012ea5a8c40ffdccb7dc5e2155c525c811f0f3641035996280fdad2d2"
        );
    }

    #[test]
    fn a_stack_state_keeps_bits_past_the_width_of_an_integer() {
        let mut state = StackState::default();
        for _ in 0..200 {
            state.push(false);
        }
        state.push(true);
        assert!(state.is_active());
        assert!(state.pop());
        assert!(!state.is_active());
        state.push(true);
        state.push(false);
        assert!(state.lexpop());
    }
}
