//! **`scripts/legacy_parser/generate.rb` の生成物。手で変更しない。**
//!
//! parser 3.3.12.0 (racc 1.8.1) の表と動作を読み出したもの。
//! 表を再構成せず gem から取得する理由は生成スクリプトを参照。
//!
//! lib/parser/lexer-F1.rb の遷移動作の翻訳。翻訳器:
//! `scripts/legacy_parser/lexer_translate.rb`.

use super::lexer::*;
use super::tokens::Tok;
use crate::legacy_parser::Level;
use crate::legacy_parser::lexer::*;
use crate::legacy_parser::parser::Fatal;
use crate::legacy_parser::value::TokenValue;

impl Lexer {
    /// `Parser::Lexer#advance` の `%% write exec;` の部分。
    #[allow(
        clippy::all,
        unused_assignments,
        unused_mut,
        unused_variables,
        unreachable_code,
        unused_parens,
        non_snake_case
    )]
    pub(crate) fn exec_main(
        &mut self,
        mut p: i64,
        pe: i64,
        eof: i64,
        mut cmd_state: bool,
    ) -> Result<i64, Fatal> {
        let mut goto_level = 0;
        let mut trans: usize = 0;
        let mut tm = 0i64;
        let mut heredoc_e = 0i64;
        let mut new_herebody_s = 0i64;
        let mut ident_ts = 0i64;
        let mut ident_te = 0i64;
        let mut dots_te = 0i64;
        let mut next_state = 0i64;
        let mut followed_by_nl = false;
        let mut nl_emitted = false;
        let mut indent = false;
        let mut dedent_body = false;
        let mut ident_tok = String::new();
        let mut type_ = String::new();
        let mut delimiter = String::new();
        let mut digits = String::new();
        let mut gvar_name = String::new();
        let mut ident = String::new();
        let mut diag_msg = "";
        let mut captures: [String; 5] = Default::default();
        'exec: loop {
            if goto_level <= 0 {
                if p == pe {
                    goto_level = L_TEST_EOF;
                    continue 'exec;
                }
                if self.cs == 0 {
                    goto_level = L_OUT;
                    continue 'exec;
                }
            }
            if goto_level <= L_RESUME {
                match FROM_STATE_ACTIONS[self.cs as usize] {
                    85 => {
                        self.ts = p;
                    }
                    _ => {}
                }
                trans = self.transition(p);
            }
            if goto_level <= L_EOF_TRANS {
                self.cs = i64::from(TRANS_TARGS[trans]);
                if TRANS_ACTIONS[trans] != 0 {
                    match TRANS_ACTIONS[trans] {
                        14 => {
                            self.newline_s = p;
                        }
                        15 => {
                            p = self.on_newline(p);
                        }
                        45 => {
                            self.sharp_s = (p - 1);
                        }
                        49 => {
                            self.emit_comment_from_range(p, pe);
                        }
                        190 => {
                            tm = p;
                        }
                        22 => {
                            tm = p;
                        }
                        24 => {
                            tm = p;
                        }
                        26 => {
                            tm = p;
                        }
                        56 => {
                            heredoc_e = p;
                        }
                        229 => {
                            tm = (p - 1);
                            diag_msg = "ivar_name";
                        }
                        232 => {
                            tm = (p - 2);
                            diag_msg = "cvar_name";
                        }
                        244 => {
                            tm = p;
                        }
                        188 => {
                            ident_tok = self.tok(self.ts, self.te);
                            ident_ts = self.ts;
                            ident_te = self.te;
                        }
                        331 => {
                            self.num_base = 16;
                            self.num_digits_s = p;
                        }
                        325 => {
                            self.num_base = 10;
                            self.num_digits_s = p;
                        }
                        328 => {
                            self.num_base = 8;
                            self.num_digits_s = p;
                        }
                        322 => {
                            self.num_base = 2;
                            self.num_digits_s = p;
                        }
                        337 => {
                            self.num_base = 10;
                            self.num_digits_s = self.ts;
                        }
                        299 => {
                            self.num_base = 8;
                            self.num_digits_s = self.ts;
                        }
                        314 => {
                            self.num_suffix_s = p;
                        }
                        307 => {
                            self.num_suffix_s = p;
                        }
                        304 => {
                            self.num_suffix_s = p;
                        }
                        74 => {
                            tm = p;
                        }
                        65 => {
                            self.te = (p + 1);
                        }
                        1 => {
                            self.te = (p + 1);
                            {
                                self.emit_global_var();
                                self.cs = (self.stack_pop());
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        87 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_global_var();
                                self.cs = (self.stack_pop());
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        89 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_class_var();
                                self.cs = (self.stack_pop());
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        88 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_instance_var();
                                self.cs = (self.stack_pop());
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        110 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::KeywordsBegin, self.ts, self.te);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        96 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        3 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                self.cs = 516;
                                {
                                    self.stack_set(self.top, self.cs);
                                    self.top += 1;
                                    self.cs = 129;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        93 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        105 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        5 => {
                            self.te = (p + 1);
                            {
                                if self.version_in(&[23]) {
                                    let multi_0 = str_slice(&self.tok(self.ts, self.te), 0, -2);
                                    let multi_1 = str_char_at(&self.tok(self.ts, self.te), -1);
                                    type_ = multi_0;
                                    delimiter = multi_1;
                                    self.push_literal(
                                        type_.clone(),
                                        delimiter.clone(),
                                        self.ts,
                                        None,
                                        false,
                                        false,
                                        false,
                                    );
                                    {
                                        self.cs = 128;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                } else {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        92 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        91 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        109 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::KeywordsBegin, self.ts, self.te);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        106 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tCONSTANT,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        108 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        103 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                self.cs = 516;
                                {
                                    self.stack_set(self.top, self.cs);
                                    self.top += 1;
                                    self.cs = 129;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        99 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        104 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        97 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        102 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        4 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        2 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        95 => match self.act {
                            4 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::KeywordsBegin, self.ts, self.te);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            5 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tCONSTANT,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            6 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            _ => {}
                        },
                        7 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tLABEL,
                                    TokenValue::from(self.tok(self.ts, (self.te - 2)).as_str()),
                                    self.ts,
                                    (self.te - 1),
                                );
                                p = (p - 1);
                                self.cs = 501;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        8 => {
                            self.te = (p + 1);
                            {
                                if ((self.version >= 31) && self.context.in_argdef) {
                                    self.emit(
                                        Tok::tBDOT3,
                                        TokenValue::from(String::from("...").as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    p -= 3;
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        112 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        111 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        114 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        113 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        6 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        120 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 276;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        119 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        118 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        130 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tCONSTANT,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = (self.arg_or_cmdarg(cmd_state));
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        121 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = (self.arg_or_cmdarg(cmd_state));
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        126 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 276;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        124 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        129 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        153 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        136 => {
                            self.te = (p + 1);
                            {
                                p = (tm - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        142 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        10 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        144 => {
                            self.te = (p + 1);
                            {
                                p = (tm - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        25 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        131 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        132 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        143 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        139 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (tm - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        141 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        135 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        134 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        152 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        11 => {
                            p = ((self.te) - 1);
                        }
                        27 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        9 => match self.act {
                            33 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (tm - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            34 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (tm - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            39 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            _ => {
                                p = ((self.te) - 1);
                            }
                        },
                        29 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 276;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        157 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        158 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 276;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        30 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 276;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        28 => match self.act {
                            46 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if self.cond.is_active() {
                                    self.emit(
                                        Tok::kDO_COND,
                                        TokenValue::from(String::from("do").as_str()),
                                        (self.te - 2),
                                        self.te,
                                    );
                                } else {
                                    self.emit(
                                        Tok::kDO,
                                        TokenValue::from(String::from("do").as_str()),
                                        (self.te - 2),
                                        self.te,
                                    );
                                }
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            47 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (self.ts - 1);
                                {
                                    self.cs = 276;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            _ => {}
                        },
                        168 => {
                            self.te = (p + 1);
                            {
                                self.emit_do(true);
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        161 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        162 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        163 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        166 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        172 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        171 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        180 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        174 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        178 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        173 => match self.act {
                            54 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            55 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (self.ts - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            _ => {}
                        },
                        42 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tUNARY_NUM,
                                    TokenValue::from(self.tok(self.ts, (self.ts + 1)).as_str()),
                                    self.ts,
                                    (self.ts + 1),
                                );
                                p = (p - 1);
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        214 => {
                            self.te = (p + 1);
                            {
                                delimiter = str_char_at(&self.tok(self.ts, self.te), 0);
                                type_ = delimiter.clone();
                                self.push_literal(
                                    type_.clone(),
                                    delimiter.clone(),
                                    self.ts,
                                    None,
                                    false,
                                    false,
                                    false,
                                );
                                p = (p - 1);
                                {
                                    self.cs = 128;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        206 => {
                            self.te = (p + 1);
                            {
                                let multi_0 = self.tok(self.ts, self.ts + 1);
                                let multi_1 = str_char_at(&self.tok(self.ts, self.te), -1);
                                type_ = multi_0;
                                delimiter = multi_1;
                                self.push_literal(
                                    type_.clone(),
                                    delimiter.clone(),
                                    self.ts,
                                    None,
                                    false,
                                    false,
                                    false,
                                );
                                {
                                    self.cs = 128;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        40 => {
                            self.te = (p + 1);
                            {
                                let multi_0 = str_slice(&self.tok(self.ts, self.te), 0, -2);
                                let multi_1 = str_char_at(&self.tok(self.ts, self.te), -1);
                                type_ = multi_0;
                                delimiter = multi_1;
                                self.push_literal(
                                    type_.clone(),
                                    delimiter.clone(),
                                    self.ts,
                                    None,
                                    false,
                                    false,
                                    false,
                                );
                                {
                                    self.cs = 128;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        227 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                p = (p - 1);
                                self.emit(
                                    Tok::tSYMBEG,
                                    TokenValue::from(self.tok(self.ts, (self.ts + 1)).as_str()),
                                    self.ts,
                                    (self.ts + 1),
                                );
                                {
                                    self.cs = 134;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        215 => {
                            self.te = (p + 1);
                            {
                                let multi_0 = self.tok(self.ts, self.te);
                                let multi_1 = str_char_at(&self.tok(self.ts, self.te), -1);
                                type_ = multi_0;
                                delimiter = multi_1;
                                self.push_literal(
                                    type_.clone(),
                                    delimiter.clone(),
                                    self.ts,
                                    None,
                                    false,
                                    false,
                                    false,
                                );
                                {
                                    self.cs = 128;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        226 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tSYMBOL,
                                    TokenValue::from(
                                        self.tok((self.ts + 1), (self.ts + 2)).as_str(),
                                    ),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        54 => {
                            self.te = (p + 1);
                            {
                                gvar_name = self.tok((self.ts + 1), self.te);
                                if (((self.version >= 33)
                                    && gvar_name.clone().starts_with(String::from("$0").as_str()))
                                    && ((gvar_name.clone().chars().count() as i64) > 2))
                                {
                                    self.diagnostic(
                                        Level::Error,
                                        format!(
                                            "`{}' is not allowed as a global variable name",
                                            gvar_name.clone()
                                        ),
                                        ((self.ts + 1), self.te),
                                    );
                                }
                                self.emit(
                                    Tok::tSYMBOL,
                                    TokenValue::from(gvar_name.clone().as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        236 => {
                            self.te = (p + 1);
                            {
                                let (multi_0, multi_1) = self.read_character_constant(self.ts)?;
                                p = multi_0;
                                next_state = multi_1;
                                p = (p - 1);
                                if self.token_queue.is_empty() {
                                    {
                                        self.cs = (next_state);
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = (next_state);
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        237 => {
                            self.te = (p + 1);
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    "incomplete character syntax".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                                return Err(Fatal);
                            }
                        }
                        216 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::PunctuationBegin, self.ts, self.te);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        37 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                if self.version_in(&[18]) {
                                    ident = self.tok(self.ts, (self.te - 2));
                                    unreachable!("1.8 の分岐");
                                    p = (p - 1);
                                    if (!(false) && self.static_env.is_declared(&ident.clone())) {
                                        self.cs = 516;
                                    } else {
                                        self.cs = (self.arg_or_cmdarg(cmd_state));
                                    }
                                } else {
                                    self.emit(
                                        Tok::tLABEL,
                                        TokenValue::from(self.tok(self.ts, (self.te - 2)).as_str()),
                                        self.ts,
                                        (self.te - 1),
                                    );
                                    self.cs = 501;
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        34 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(ident_tok.clone().as_str()),
                                    ident_ts,
                                    ident_te,
                                );
                                p = (ident_te - 1);
                                if ((!(false) && self.static_env.is_declared(&ident_tok.clone()))
                                    && (self.version < 25))
                                {
                                    self.cs = 247;
                                } else {
                                    self.cs = 307;
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        200 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                self.cs_before_block_comment = self.cs;
                                {
                                    self.cs = 710;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        41 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        183 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        210 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tUNARY_NUM,
                                    TokenValue::from(self.tok(self.ts, (self.ts + 1)).as_str()),
                                    self.ts,
                                    (self.ts + 1),
                                );
                                p = (p - 1);
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        209 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tSTAR,
                                    TokenValue::from(String::from("*").as_str()),
                                    self.ts,
                                    self.te,
                                );
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        205 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    "unterminated string meets end of file".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                                return Err(Fatal);
                            }
                        }
                        234 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.diagnostic(
                                    Level::Error,
                                    "unterminated heredoc id".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                            }
                        }
                        217 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                gvar_name = self.tok((self.ts + 1), self.te);
                                if (((self.version >= 33)
                                    && gvar_name.clone().starts_with(String::from("$0").as_str()))
                                    && ((gvar_name.clone().chars().count() as i64) > 2))
                                {
                                    self.diagnostic(
                                        Level::Error,
                                        format!(
                                            "`{}' is not allowed as a global variable name",
                                            gvar_name.clone()
                                        ),
                                        ((self.ts + 1), self.te),
                                    );
                                }
                                self.emit(
                                    Tok::tSYMBOL,
                                    TokenValue::from(gvar_name.clone().as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        230 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_colon_with_digits(p, tm, diag_msg);
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        235 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    "incomplete character syntax".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                                return Err(Fatal);
                            }
                        }
                        207 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::PunctuationBegin, self.ts, self.te);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        211 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                if (self.version >= 27) {
                                    self.emit(
                                        Tok::tBDOT2,
                                        TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                } else {
                                    self.emit(
                                        Tok::tDOT2,
                                        TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                }
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        212 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                followed_by_nl = ((self.te - 1) == self.newline_s);
                                nl_emitted = false;
                                dots_te = (if followed_by_nl {
                                    (self.te - 1)
                                } else {
                                    self.te
                                });
                                if (self.version >= 30) {
                                    if (!self.lambda_stack.is_empty()
                                        && ((self.lambda_stack.last().copied().unwrap_or(NIL) + 1)
                                            == self.paren_nest))
                                    {
                                        self.emit(
                                            Tok::tDOT3,
                                            TokenValue::from(String::from("...").as_str()),
                                            self.ts,
                                            dots_te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::tBDOT3,
                                            TokenValue::from(String::from("...").as_str()),
                                            self.ts,
                                            dots_te,
                                        );
                                        if (((self.version >= 31) && followed_by_nl)
                                            && self.context.in_argdef)
                                        {
                                            self.emit(
                                                Tok::tNL,
                                                TokenValue::from(
                                                    (self.te - 1).to_string().as_str(),
                                                ),
                                                self.te,
                                                self.te,
                                            );
                                            nl_emitted = true;
                                        }
                                    }
                                } else if (self.version >= 27) {
                                    self.emit(
                                        Tok::tBDOT3,
                                        TokenValue::from(String::from("...").as_str()),
                                        self.ts,
                                        dots_te,
                                    );
                                } else {
                                    self.emit(
                                        Tok::tDOT3,
                                        TokenValue::from(String::from("...").as_str()),
                                        self.ts,
                                        dots_te,
                                    );
                                }
                                if (followed_by_nl && !(nl_emitted)) {
                                    p = (p - 1);
                                }
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        187 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                if (!(false)
                                    && self.static_env.is_declared(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else if ((self.version >= 32)
                                    && is_numparam_name(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        197 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        199 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                self.cs_before_block_comment = self.cs;
                                {
                                    self.cs = 710;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        202 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        39 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    "unterminated string meets end of file".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                                return Err(Fatal);
                            }
                        }
                        58 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.diagnostic(
                                    Level::Error,
                                    "unterminated heredoc id".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                            }
                        }
                        33 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                if (!(false)
                                    && self.static_env.is_declared(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else if ((self.version >= 32)
                                    && is_numparam_name(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        38 => {
                            p = ((self.te) - 1);
                        }
                        53 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        36 => match self.act {
                            60 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tUNARY_NUM,
                                    TokenValue::from(self.tok(self.ts, (self.ts + 1)).as_str()),
                                    self.ts,
                                    (self.ts + 1),
                                );
                                p = (p - 1);
                                self.cs = 516;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            67 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.diagnostic(
                                    Level::Error,
                                    "unterminated heredoc id".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                            }
                            76 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if (self.version >= 27) {
                                    self.emit(
                                        Tok::tPIPE,
                                        TokenValue::from(self.tok(self.ts, (self.ts + 1)).as_str()),
                                        self.ts,
                                        (self.ts + 1),
                                    );
                                    p = (p - 1);
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    p -= 2;
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                            80 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::PunctuationBegin, self.ts, self.te);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            81 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::kRESCUE,
                                    TokenValue::from(String::from("rescue").as_str()),
                                    self.ts,
                                    tm,
                                );
                                p = (tm - 1);
                                self.cs = 321;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            82 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::KeywordsBegin, self.ts, self.te);
                                self.command_start = true;
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            86 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            87 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                if (!(false)
                                    && self.static_env.is_declared(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else if ((self.version >= 32)
                                    && is_numparam_name(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                            91 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                            _ => {}
                        },
                        247 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        248 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        249 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        253 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        61 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        257 => {
                            self.te = (p + 1);
                            {
                                self.push_literal(
                                    self.tok(self.ts, self.te),
                                    self.tok(self.ts, self.te),
                                    self.ts,
                                    None,
                                    false,
                                    false,
                                    false,
                                );
                                {
                                    self.cs = 128;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        256 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        255 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        259 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        258 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        60 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                p = (p - 1);
                                {
                                    self.cs = 345;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        292 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tLAMBDA,
                                    TokenValue::from(String::from("->").as_str()),
                                    self.ts,
                                    (self.ts + 2),
                                );
                                self.lambda_stack.push(self.paren_nest);
                                self.cs = 247;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        71 => {
                            self.te = (p + 1);
                            {
                                self.emit_singleton_class();
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        267 => {
                            self.te = (p + 1);
                            {
                                let multi_0 = self.tok(self.ts, self.te);
                                let multi_1 = str_char_at(&self.tok(self.ts, self.te), -1);
                                type_ = multi_0;
                                delimiter = multi_1;
                                self.push_literal(
                                    type_.clone(),
                                    delimiter.clone(),
                                    self.ts,
                                    None,
                                    false,
                                    false,
                                    true,
                                );
                                {
                                    self.cs = 128;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        63 => {
                            self.te = (p + 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.stack_set(self.top, self.cs);
                                    self.top += 1;
                                    self.cs = 129;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        288 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 255;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        341 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        281 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        286 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tOP_ASGN,
                                    TokenValue::from(self.tok(self.ts, (self.te - 1)).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        272 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tEH,
                                    TokenValue::from(String::from("?").as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        269 => {
                            self.te = (p + 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        271 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tSEMI,
                                    TokenValue::from(String::from(";").as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.command_start = true;
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        346 => {
                            self.te = (p + 1);
                            {
                                self.diagnostic(
                                    Level::Error,
                                    "bare backslash only allowed before newline".to_owned(),
                                    (self.ts, (self.ts + 1)),
                                );
                                p = (p - 1);
                            }
                        }
                        266 => {
                            self.te = (p + 1);
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    format!(
                                        "unexpected `{}'",
                                        str_slice(
                                            &ruby_inspect(
                                                &self.tok(self.ts, self.te),
                                                self.encoding
                                            ),
                                            1,
                                            -2
                                        )
                                    ),
                                    (self.ts, self.te),
                                );
                                return Err(Fatal);
                            }
                        }
                        265 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        357 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.cs = 134;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        355 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_singleton_class();
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        354 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.command_start = true;
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        296 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.diagnostic(
                                    Level::Error,
                                    "no .<digit> floating literal anymore; put 0 before dot"
                                        .to_owned(),
                                    (self.ts, self.te),
                                );
                            }
                        }
                        343 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tCONSTANT,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = (self.arg_or_cmdarg(cmd_state));
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        285 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (self.ts - 1);
                                {
                                    self.stack_set(self.top, self.cs);
                                    self.top += 1;
                                    self.cs = 129;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        293 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 255;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        349 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                if (!(false)
                                    && self.static_env.is_declared(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else if ((self.version >= 32)
                                    && is_numparam_name(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        291 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        287 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        280 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        294 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        278 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        284 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    format!(
                                        "unexpected `{}'",
                                        str_slice(
                                            &ruby_inspect(
                                                &self.tok(self.ts, self.te),
                                                self.encoding
                                            ),
                                            1,
                                            -2
                                        )
                                    ),
                                    (self.ts, self.te),
                                );
                                return Err(Fatal);
                            }
                        }
                        69 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                digits = self.numeric_literal_int();
                                if self.version_in(&[18, 19, 20]) {
                                    self.emit(
                                        Tok::tINTEGER,
                                        TokenValue::from(digits.clone().as_str()),
                                        self.ts,
                                        self.num_suffix_s,
                                    );
                                    p = (self.num_suffix_s - 1);
                                } else {
                                    p = self.num_xfrm_call(digits.clone(), p);
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        64 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.diagnostic(
                                    Level::Error,
                                    "no .<digit> floating literal anymore; put 0 before dot"
                                        .to_owned(),
                                    (self.ts, self.te),
                                );
                            }
                        }
                        68 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                digits = self.tok(self.ts, self.num_suffix_s);
                                if self.version_in(&[18, 19, 20]) {
                                    self.emit(
                                        Tok::tFLOAT,
                                        TokenValue::from(digits.clone().as_str()),
                                        self.ts,
                                        self.num_suffix_s,
                                    );
                                    p = (self.num_suffix_s - 1);
                                } else {
                                    p = self.num_xfrm_call(digits.clone(), p);
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        62 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.diagnostic(
                                    Level::Fatal,
                                    format!(
                                        "unexpected `{}'",
                                        str_slice(
                                            &ruby_inspect(
                                                &self.tok(self.ts, self.te),
                                                self.encoding
                                            ),
                                            1,
                                            -2
                                        )
                                    ),
                                    (self.ts, self.te),
                                );
                                return Err(Fatal);
                            }
                        }
                        66 => match self.act {
                            104 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if (self.lambda_stack.last().copied().unwrap_or(NIL)
                                    == self.paren_nest)
                                {
                                    self.lambda_stack.pop();
                                    if (self.tok(self.ts, self.te) == String::from("{")) {
                                        self.emit(
                                            Tok::tLAMBEG,
                                            TokenValue::from(String::from("{").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::kDO_LAMBDA,
                                            TokenValue::from(String::from("do").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    }
                                } else {
                                    if (self.tok(self.ts, self.te) == String::from("{")) {
                                        self.emit(
                                            Tok::tLCURLY,
                                            TokenValue::from(String::from("{").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    } else {
                                        self.emit_do(false);
                                    }
                                }
                                if (self.tok(self.ts, self.te) == String::from("{")) {
                                    self.paren_nest += 1;
                                }
                                self.command_start = true;
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            105 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.cs = 134;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            106 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_singleton_class();
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            107 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            108 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.command_start = true;
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            109 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                self.cs = 321;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            110 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                if (self.version_in(&[18])
                                    && (self.tok(self.ts, self.te) == String::from("not")))
                                {
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = 276;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                            111 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if self.version_in(&[18]) {
                                    self.emit(
                                        Tok::tIDENTIFIER,
                                        TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    if !(!(false)
                                        && self.static_env.is_declared(&self.tok(self.ts, self.te)))
                                    {
                                        self.cs = (self.arg_or_cmdarg(cmd_state));
                                    }
                                } else {
                                    self.emit(
                                        Tok::k__ENCODING__,
                                        TokenValue::from(String::from("__ENCODING__").as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            112 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Keywords, self.ts, self.te);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            113 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                digits = self.numeric_literal_int();
                                if self.version_in(&[18, 19, 20]) {
                                    self.emit(
                                        Tok::tINTEGER,
                                        TokenValue::from(digits.clone().as_str()),
                                        self.ts,
                                        self.num_suffix_s,
                                    );
                                    p = (self.num_suffix_s - 1);
                                } else {
                                    p = self.num_xfrm_call(digits.clone(), p);
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            115 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if self.version_in(&[18, 19, 20]) {
                                    self.diagnostic(
                                        Level::Error,
                                        format!(
                                            "trailing `{}' in number",
                                            self.tok((self.te - 1), self.te)
                                        ),
                                        ((self.te - 1), self.te),
                                    );
                                } else {
                                    self.emit(
                                        Tok::tINTEGER,
                                        TokenValue::from(self.tok(self.ts, (self.te - 1)).as_str()),
                                        self.ts,
                                        (self.te - 1),
                                    );
                                    p = (p - 1);
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                            116 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if self.version_in(&[18, 19, 20]) {
                                    self.diagnostic(
                                        Level::Error,
                                        format!(
                                            "trailing `{}' in number",
                                            self.tok((self.te - 1), self.te)
                                        ),
                                        ((self.te - 1), self.te),
                                    );
                                } else {
                                    self.emit(
                                        Tok::tFLOAT,
                                        TokenValue::from(self.tok(self.ts, (self.te - 1)).as_str()),
                                        self.ts,
                                        (self.te - 1),
                                    );
                                    p = (p - 1);
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                            117 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                digits = self.tok(self.ts, self.num_suffix_s);
                                if self.version_in(&[18, 19, 20]) {
                                    self.emit(
                                        Tok::tFLOAT,
                                        TokenValue::from(digits.clone().as_str()),
                                        self.ts,
                                        self.num_suffix_s,
                                    );
                                    p = (self.num_suffix_s - 1);
                                } else {
                                    p = self.num_xfrm_call(digits.clone(), p);
                                }
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            119 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tCONSTANT,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                self.cs = (self.arg_or_cmdarg(cmd_state));
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            123 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tIDENTIFIER,
                                    TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                    self.ts,
                                    self.te,
                                );
                                if (!(false)
                                    && self.static_env.is_declared(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else if ((self.version >= 32)
                                    && is_numparam_name(&self.tok(self.ts, self.te)))
                                {
                                    self.cs = 247;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                            124 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if (tm == self.te) {
                                    self.emit(
                                        Tok::tFID,
                                        TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                } else {
                                    self.emit(
                                        Tok::tIDENTIFIER,
                                        TokenValue::from(self.tok(self.ts, tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                }
                                self.cs = 276;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            126 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 508;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            127 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit_table(Table::Punctuation, self.ts, self.te);
                                self.cs = 345;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            _ => {}
                        },
                        368 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tNL,
                                    TokenValue::Nil,
                                    self.newline_s,
                                    (self.newline_s + 1),
                                );
                                if (self.version < 27) {
                                    p = (p - 1);
                                    self.cs = 710;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.emit(
                                        Tok::tBDOT3,
                                        TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        80 => {
                            self.te = (p + 1);
                            {
                                p = (tm - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        362 => {
                            self.te = (p + 1);
                            {
                                self.emit(
                                    Tok::tNL,
                                    TokenValue::Nil,
                                    self.newline_s,
                                    (self.newline_s + 1),
                                );
                                p = (p - 1);
                                self.cs = 710;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        365 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                if (self.version < 27) {
                                    self.emit(
                                        Tok::tNL,
                                        TokenValue::Nil,
                                        self.newline_s,
                                        (self.newline_s + 1),
                                    );
                                    p = (p - 1);
                                    self.cs = 710;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        367 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tNL,
                                    TokenValue::Nil,
                                    self.newline_s,
                                    (self.newline_s + 1),
                                );
                                if (self.version < 27) {
                                    p = (p - 1);
                                    self.cs = 710;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                } else {
                                    self.emit(
                                        Tok::tBDOT2,
                                        TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        366 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                p = (tm - 1);
                                {
                                    self.cs = 516;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        364 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit(
                                    Tok::tNL,
                                    TokenValue::Nil,
                                    self.newline_s,
                                    (self.newline_s + 1),
                                );
                                p = (p - 1);
                                self.cs = 710;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        75 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                if (self.version < 27) {
                                    self.emit(
                                        Tok::tNL,
                                        TokenValue::Nil,
                                        self.newline_s,
                                        (self.newline_s + 1),
                                    );
                                    p = (p - 1);
                                    self.cs = 710;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        72 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                self.emit(
                                    Tok::tNL,
                                    TokenValue::Nil,
                                    self.newline_s,
                                    (self.newline_s + 1),
                                );
                                p = (p - 1);
                                self.cs = 710;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        76 => match self.act {
                            140 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                if (self.version < 27) {
                                    self.emit(
                                        Tok::tNL,
                                        TokenValue::Nil,
                                        self.newline_s,
                                        (self.newline_s + 1),
                                    );
                                    p = (p - 1);
                                    self.cs = 710;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                            144 => {
                                {
                                    p = ((self.te) - 1);
                                }
                                self.emit(
                                    Tok::tNL,
                                    TokenValue::Nil,
                                    self.newline_s,
                                    (self.newline_s + 1),
                                );
                                p = (p - 1);
                                self.cs = 710;
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                            _ => {}
                        },
                        371 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.emit_comment(self.eq_begin_s, self.te);
                                {
                                    self.cs = (self.cs_before_block_comment);
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        370 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.diagnostic(Level::Fatal, "embedded document meets end of file (and they embark on a romantic journey)".to_owned(), (self.eq_begin_s, (self.eq_begin_s + (String::from("=begin").chars().count() as i64))));
                                return Err(Fatal);
                            }
                        }
                        381 => {
                            self.te = (p + 1);
                            {
                                self.eq_begin_s = self.ts;
                                {
                                    self.cs = 704;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        82 => {
                            self.te = (p + 1);
                            {
                                p = (pe - 3);
                            }
                        }
                        373 => {
                            self.te = (p + 1);
                            {
                                cmd_state = true;
                                p = (p - 1);
                                {
                                    self.cs = 508;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        374 => {
                            self.te = (p + 1);
                            {
                                p = (p - 1);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        375 => {
                            self.te = p;
                            p = (p - 1);
                        }
                        380 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                self.eq_begin_s = self.ts;
                                {
                                    self.cs = 704;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        379 => {
                            self.te = p;
                            p = (p - 1);
                            {
                                cmd_state = true;
                                p = (p - 1);
                                {
                                    self.cs = 508;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        81 => {
                            {
                                p = ((self.te) - 1);
                            }
                            {
                                cmd_state = true;
                                p = (p - 1);
                                {
                                    self.cs = 508;
                                    goto_level = L_AGAIN;
                                    continue 'exec;
                                }
                            }
                        }
                        86 => {
                            self.te = (p + 1);
                            {
                                let (multi_0, multi_1) = self.strings_advance(p)?;
                                p = multi_0;
                                next_state = multi_1;
                                p = (p - 1);
                                self.cs = (next_state);
                                {
                                    p += 1;
                                    goto_level = L_OUT;
                                    continue 'exec;
                                }
                            }
                        }
                        52 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                        }
                        154 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        145 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    p = (tm - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        137 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        213 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    followed_by_nl = ((self.te - 1) == self.newline_s);
                                    nl_emitted = false;
                                    dots_te = (if followed_by_nl {
                                        (self.te - 1)
                                    } else {
                                        self.te
                                    });
                                    if (self.version >= 30) {
                                        if (!self.lambda_stack.is_empty()
                                            && ((self.lambda_stack.last().copied().unwrap_or(NIL)
                                                + 1)
                                                == self.paren_nest))
                                        {
                                            self.emit(
                                                Tok::tDOT3,
                                                TokenValue::from(String::from("...").as_str()),
                                                self.ts,
                                                dots_te,
                                            );
                                        } else {
                                            self.emit(
                                                Tok::tBDOT3,
                                                TokenValue::from(String::from("...").as_str()),
                                                self.ts,
                                                dots_te,
                                            );
                                            if (((self.version >= 31) && followed_by_nl)
                                                && self.context.in_argdef)
                                            {
                                                self.emit(
                                                    Tok::tNL,
                                                    TokenValue::from(
                                                        (self.te - 1).to_string().as_str(),
                                                    ),
                                                    self.te,
                                                    self.te,
                                                );
                                                nl_emitted = true;
                                            }
                                        }
                                    } else if (self.version >= 27) {
                                        self.emit(
                                            Tok::tBDOT3,
                                            TokenValue::from(String::from("...").as_str()),
                                            self.ts,
                                            dots_te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::tDOT3,
                                            TokenValue::from(String::from("...").as_str()),
                                            self.ts,
                                            dots_te,
                                        );
                                    }
                                    if (followed_by_nl && !(nl_emitted)) {
                                        p = (p - 1);
                                    }
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        201 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    p = (self.ts - 1);
                                    self.cs_before_block_comment = self.cs;
                                    {
                                        self.cs = 710;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        295 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    if (self.paren_nest == 0) {}
                                    self.emit(
                                        Tok::tDOT3,
                                        TokenValue::from(String::from("...").as_str()),
                                        self.ts,
                                        (self.te - 1),
                                    );
                                    p = (p - 1);
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        372 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    self.emit_comment(self.eq_begin_s, self.te);
                                    {
                                        self.cs = (self.cs_before_block_comment);
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        369 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                            }
                        }
                        382 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    self.eq_begin_s = self.ts;
                                    {
                                        self.cs = 704;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        83 => {
                            {
                                self.newline_s = p;
                            }
                            {
                                self.te = (p + 1);
                                {
                                    p = (pe - 3);
                                }
                            }
                        }
                        317 => {
                            {
                                self.num_xfrm = NumXfrm::Rational;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        315 => {
                            {
                                self.num_xfrm = NumXfrm::Imaginary;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        320 => {
                            {
                                self.num_xfrm = NumXfrm::ImaginaryRational;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        318 => {
                            {
                                self.num_xfrm = NumXfrm::IntegerRe;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        316 => {
                            {
                                self.num_xfrm = NumXfrm::IntegerIf;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        319 => {
                            {
                                self.num_xfrm = NumXfrm::IntegerRescue;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        308 => {
                            {
                                self.num_xfrm = NumXfrm::ImaginaryFloat;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        309 => {
                            {
                                self.num_xfrm = NumXfrm::FloatIf;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        310 => {
                            {
                                self.num_xfrm = NumXfrm::Rational;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        312 => {
                            {
                                self.num_xfrm = NumXfrm::ImaginaryRational;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        311 => {
                            {
                                self.num_xfrm = NumXfrm::FloatRescue;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        147 => {
                            {
                                self.e_lbrace();
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if (self.lambda_stack.last().copied().unwrap_or(NIL)
                                        == self.paren_nest)
                                    {
                                        self.lambda_stack.pop();
                                        self.emit(
                                            Tok::tLAMBEG,
                                            TokenValue::from(String::from("{").as_str()),
                                            (self.te - 1),
                                            self.te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::tLCURLY,
                                            TokenValue::from(String::from("{").as_str()),
                                            (self.te - 1),
                                            self.te,
                                        );
                                    }
                                    self.command_start = true;
                                    self.paren_nest += 1;
                                    self.cs = 508;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        169 => {
                            {
                                self.e_lbrace();
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if (self.lambda_stack.last().copied().unwrap_or(NIL)
                                        == self.paren_nest)
                                    {
                                        self.lambda_stack.pop();
                                        self.emit(
                                            Tok::tLAMBEG,
                                            TokenValue::from(String::from("{").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::tLBRACE_ARG,
                                            TokenValue::from(String::from("{").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    }
                                    self.paren_nest += 1;
                                    self.command_start = true;
                                    self.cs = 508;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        245 => {
                            {
                                self.e_lbrace();
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if (self.lambda_stack.last().copied().unwrap_or(NIL)
                                        == self.paren_nest)
                                    {
                                        self.lambda_stack.pop();
                                        self.command_start = true;
                                        self.emit(
                                            Tok::tLAMBEG,
                                            TokenValue::from(String::from("{").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::tLBRACE,
                                            TokenValue::from(String::from("{").as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    }
                                    self.paren_nest += 1;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        360 => {
                            {
                                self.e_lbrace();
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if (self.lambda_stack.last().copied().unwrap_or(NIL)
                                        == self.paren_nest)
                                    {
                                        self.lambda_stack.pop();
                                        if (self.tok(self.ts, self.te) == String::from("{")) {
                                            self.emit(
                                                Tok::tLAMBEG,
                                                TokenValue::from(String::from("{").as_str()),
                                                self.ts,
                                                self.te,
                                            );
                                        } else {
                                            self.emit(
                                                Tok::kDO_LAMBDA,
                                                TokenValue::from(String::from("do").as_str()),
                                                self.ts,
                                                self.te,
                                            );
                                        }
                                    } else {
                                        if (self.tok(self.ts, self.te) == String::from("{")) {
                                            self.emit(
                                                Tok::tLCURLY,
                                                TokenValue::from(String::from("{").as_str()),
                                                self.ts,
                                                self.te,
                                            );
                                        } else {
                                            self.emit_do(false);
                                        }
                                    }
                                    if (self.tok(self.ts, self.te) == String::from("{")) {
                                        self.paren_nest += 1;
                                    }
                                    self.command_start = true;
                                    self.cs = 508;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        361 => {
                            {
                                if self.close_interp_on_current_literal(p) {
                                    p = (p - 1);
                                    self.cs = 128;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                                self.paren_nest -= 1;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit_rbrace_rparen_rbrack();
                                    if ((self.tok(self.ts, self.te) == String::from("}"))
                                        || (self.tok(self.ts, self.te) == String::from("]")))
                                    {
                                        if (self.version >= 25) {
                                            self.cs = 516;
                                        } else {
                                            self.cs = 313;
                                        }
                                    } else {
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        43 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.newline_s = p;
                            }
                        }
                        16 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                tm = p;
                            }
                        }
                        18 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                tm = p;
                            }
                        }
                        20 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                tm = p;
                            }
                        }
                        98 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        117 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        125 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        19 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = (p + 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        156 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        148 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (p - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        167 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        179 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        175 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (p - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        44 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = (p + 1);
                                {
                                    self.emit(
                                        Tok::tUNARY_NUM,
                                        TokenValue::from(self.tok(self.ts, (self.ts + 1)).as_str()),
                                        self.ts,
                                        (self.ts + 1),
                                    );
                                    p = (p - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        35 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = (p + 1);
                                {
                                    self.emit(
                                        Tok::tIDENTIFIER,
                                        TokenValue::from(ident_tok.clone().as_str()),
                                        ident_ts,
                                        ident_te,
                                    );
                                    p = (ident_te - 1);
                                    if ((!(false)
                                        && self.static_env.is_declared(&ident_tok.clone()))
                                        && (self.version < 25))
                                    {
                                        self.cs = 247;
                                    } else {
                                        self.cs = 307;
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        198 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        254 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        250 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if self.context.in_kwarg {
                                        p = (p - 1);
                                        {
                                            self.cs = 516;
                                            goto_level = L_AGAIN;
                                            continue 'exec;
                                        }
                                    } else {
                                        {
                                            self.cs = 710;
                                            goto_level = L_AGAIN;
                                            continue 'exec;
                                        }
                                    }
                                }
                            }
                        }
                        263 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        260 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    {
                                        self.cs = 710;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        347 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        279 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    {
                                        self.cs = 696;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        376 => {
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        46 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                        }
                        50 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.newline_s = p;
                            }
                        }
                        101 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        116 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        128 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        150 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        165 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        177 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        204 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        252 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        262 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        283 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        378 => {
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        122 => {
                            {
                                tm = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tFID,
                                        TokenValue::from(self.tok(self.ts, tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    p = (tm - 1);
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        218 => {
                            {
                                tm = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        189 => {
                            {
                                tm = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        276 => {
                            {
                                tm = p;
                            }
                            {
                                match self.act {
                                    104 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        if (self.lambda_stack.last().copied().unwrap_or(NIL)
                                            == self.paren_nest)
                                        {
                                            self.lambda_stack.pop();
                                            if (self.tok(self.ts, self.te) == String::from("{")) {
                                                self.emit(
                                                    Tok::tLAMBEG,
                                                    TokenValue::from(String::from("{").as_str()),
                                                    self.ts,
                                                    self.te,
                                                );
                                            } else {
                                                self.emit(
                                                    Tok::kDO_LAMBDA,
                                                    TokenValue::from(String::from("do").as_str()),
                                                    self.ts,
                                                    self.te,
                                                );
                                            }
                                        } else {
                                            if (self.tok(self.ts, self.te) == String::from("{")) {
                                                self.emit(
                                                    Tok::tLCURLY,
                                                    TokenValue::from(String::from("{").as_str()),
                                                    self.ts,
                                                    self.te,
                                                );
                                            } else {
                                                self.emit_do(false);
                                            }
                                        }
                                        if (self.tok(self.ts, self.te) == String::from("{")) {
                                            self.paren_nest += 1;
                                        }
                                        self.command_start = true;
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    105 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Keywords, self.ts, self.te);
                                        self.cs = 134;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    106 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_singleton_class();
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    107 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Keywords, self.ts, self.te);
                                        self.cs = 345;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    108 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Keywords, self.ts, self.te);
                                        self.command_start = true;
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    109 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Keywords, self.ts, self.te);
                                        self.cs = 321;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    110 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Keywords, self.ts, self.te);
                                        if (self.version_in(&[18])
                                            && (self.tok(self.ts, self.te) == String::from("not")))
                                        {
                                            self.cs = 345;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        } else {
                                            self.cs = 276;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        }
                                    }
                                    111 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        if self.version_in(&[18]) {
                                            self.emit(
                                                Tok::tIDENTIFIER,
                                                TokenValue::from(
                                                    self.tok(self.ts, self.te).as_str(),
                                                ),
                                                self.ts,
                                                self.te,
                                            );
                                            if !(!(false)
                                                && self
                                                    .static_env
                                                    .is_declared(&self.tok(self.ts, self.te)))
                                            {
                                                self.cs = (self.arg_or_cmdarg(cmd_state));
                                            }
                                        } else {
                                            self.emit(
                                                Tok::k__ENCODING__,
                                                TokenValue::from(
                                                    String::from("__ENCODING__").as_str(),
                                                ),
                                                self.ts,
                                                self.te,
                                            );
                                        }
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    112 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Keywords, self.ts, self.te);
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    113 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        digits = self.numeric_literal_int();
                                        if self.version_in(&[18, 19, 20]) {
                                            self.emit(
                                                Tok::tINTEGER,
                                                TokenValue::from(digits.clone().as_str()),
                                                self.ts,
                                                self.num_suffix_s,
                                            );
                                            p = (self.num_suffix_s - 1);
                                        } else {
                                            p = self.num_xfrm_call(digits.clone(), p);
                                        }
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    115 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        if self.version_in(&[18, 19, 20]) {
                                            self.diagnostic(
                                                Level::Error,
                                                format!(
                                                    "trailing `{}' in number",
                                                    self.tok((self.te - 1), self.te)
                                                ),
                                                ((self.te - 1), self.te),
                                            );
                                        } else {
                                            self.emit(
                                                Tok::tINTEGER,
                                                TokenValue::from(
                                                    self.tok(self.ts, (self.te - 1)).as_str(),
                                                ),
                                                self.ts,
                                                (self.te - 1),
                                            );
                                            p = (p - 1);
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        }
                                    }
                                    116 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        if self.version_in(&[18, 19, 20]) {
                                            self.diagnostic(
                                                Level::Error,
                                                format!(
                                                    "trailing `{}' in number",
                                                    self.tok((self.te - 1), self.te)
                                                ),
                                                ((self.te - 1), self.te),
                                            );
                                        } else {
                                            self.emit(
                                                Tok::tFLOAT,
                                                TokenValue::from(
                                                    self.tok(self.ts, (self.te - 1)).as_str(),
                                                ),
                                                self.ts,
                                                (self.te - 1),
                                            );
                                            p = (p - 1);
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        }
                                    }
                                    117 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        digits = self.tok(self.ts, self.num_suffix_s);
                                        if self.version_in(&[18, 19, 20]) {
                                            self.emit(
                                                Tok::tFLOAT,
                                                TokenValue::from(digits.clone().as_str()),
                                                self.ts,
                                                self.num_suffix_s,
                                            );
                                            p = (self.num_suffix_s - 1);
                                        } else {
                                            p = self.num_xfrm_call(digits.clone(), p);
                                        }
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    119 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit(
                                            Tok::tCONSTANT,
                                            TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                        self.cs = (self.arg_or_cmdarg(cmd_state));
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    123 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit(
                                            Tok::tIDENTIFIER,
                                            TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                        if (!(false)
                                            && self
                                                .static_env
                                                .is_declared(&self.tok(self.ts, self.te)))
                                        {
                                            self.cs = 247;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        } else if ((self.version >= 32)
                                            && is_numparam_name(&self.tok(self.ts, self.te)))
                                        {
                                            self.cs = 247;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        } else {
                                            self.cs = (self.arg_or_cmdarg(cmd_state));
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        }
                                    }
                                    124 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        if (tm == self.te) {
                                            self.emit(
                                                Tok::tFID,
                                                TokenValue::from(
                                                    self.tok(self.ts, self.te).as_str(),
                                                ),
                                                self.ts,
                                                self.te,
                                            );
                                        } else {
                                            self.emit(
                                                Tok::tIDENTIFIER,
                                                TokenValue::from(self.tok(self.ts, tm).as_str()),
                                                self.ts,
                                                tm,
                                            );
                                            p = (tm - 1);
                                        }
                                        self.cs = 276;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    126 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Punctuation, self.ts, self.te);
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    127 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::Punctuation, self.ts, self.te);
                                        self.cs = 345;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        123 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tFID,
                                        TokenValue::from(self.tok(self.ts, tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    self.cs = (self.arg_or_cmdarg(cmd_state));
                                    p = (tm - 1);
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        219 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        191 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        277 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if (tm == self.te) {
                                        self.emit(
                                            Tok::tFID,
                                            TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                    } else {
                                        self.emit(
                                            Tok::tIDENTIFIER,
                                            TokenValue::from(self.tok(self.ts, tm).as_str()),
                                            self.ts,
                                            tm,
                                        );
                                        p = (tm - 1);
                                    }
                                    self.cs = 276;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        220 => {
                            {
                                tm = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        192 => {
                            {
                                tm = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        221 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        193 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        225 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        196 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        224 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        195 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                match self.act {
                                    60 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit(
                                            Tok::tUNARY_NUM,
                                            TokenValue::from(
                                                self.tok(self.ts, (self.ts + 1)).as_str(),
                                            ),
                                            self.ts,
                                            (self.ts + 1),
                                        );
                                        p = (p - 1);
                                        self.cs = 516;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    67 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.diagnostic(
                                            Level::Error,
                                            "unterminated heredoc id".to_owned(),
                                            (self.ts, (self.ts + 1)),
                                        );
                                    }
                                    76 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        if (self.version >= 27) {
                                            self.emit(
                                                Tok::tPIPE,
                                                TokenValue::from(
                                                    self.tok(self.ts, (self.ts + 1)).as_str(),
                                                ),
                                                self.ts,
                                                (self.ts + 1),
                                            );
                                            p = (p - 1);
                                            self.cs = 345;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        } else {
                                            p -= 2;
                                            {
                                                self.cs = 516;
                                                goto_level = L_AGAIN;
                                                continue 'exec;
                                            }
                                        }
                                    }
                                    80 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::PunctuationBegin, self.ts, self.te);
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    81 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit(
                                            Tok::kRESCUE,
                                            TokenValue::from(String::from("rescue").as_str()),
                                            self.ts,
                                            tm,
                                        );
                                        p = (tm - 1);
                                        self.cs = 321;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    82 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit_table(Table::KeywordsBegin, self.ts, self.te);
                                        self.command_start = true;
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                    86 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        p = (self.ts - 1);
                                        {
                                            self.cs = 516;
                                            goto_level = L_AGAIN;
                                            continue 'exec;
                                        }
                                    }
                                    87 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        self.emit(
                                            Tok::tIDENTIFIER,
                                            TokenValue::from(self.tok(self.ts, self.te).as_str()),
                                            self.ts,
                                            self.te,
                                        );
                                        if (!(false)
                                            && self
                                                .static_env
                                                .is_declared(&self.tok(self.ts, self.te)))
                                        {
                                            self.cs = 247;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        } else if ((self.version >= 32)
                                            && is_numparam_name(&self.tok(self.ts, self.te)))
                                        {
                                            self.cs = 247;
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        } else {
                                            self.cs = (self.arg_or_cmdarg(cmd_state));
                                            {
                                                p += 1;
                                                goto_level = L_OUT;
                                                continue 'exec;
                                            }
                                        }
                                    }
                                    91 => {
                                        {
                                            p = ((self.te) - 1);
                                        }
                                        p = (self.ts - 1);
                                        {
                                            self.cs = 516;
                                            goto_level = L_AGAIN;
                                            continue 'exec;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        222 => {
                            {
                                tm = (p - 3);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        194 => {
                            {
                                tm = (p - 3);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    p = (self.ts - 1);
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        223 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tSYMBOL,
                                        TokenValue::from(self.tok((self.ts + 1), tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        342 => {
                            {
                                tm = (p - 2);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tCONSTANT,
                                        TokenValue::from(self.tok(self.ts, tm).as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        146 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tLBRACK,
                                        TokenValue::from(String::from("[").as_str()),
                                        (self.te - 1),
                                        self.te,
                                    );
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        238 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tLBRACK,
                                        TokenValue::from(String::from("[").as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        345 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tLBRACK2,
                                        TokenValue::from(String::from("[").as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        348 => {
                            {
                                self.paren_nest -= 1;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit_rbrace_rparen_rbrack();
                                    if ((self.tok(self.ts, self.te) == String::from("}"))
                                        || (self.tok(self.ts, self.te) == String::from("]")))
                                    {
                                        if (self.version >= 25) {
                                            self.cs = 516;
                                        } else {
                                            self.cs = 313;
                                        }
                                    } else {
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        138 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                                if self.version_in(&[18]) {
                                    self.command_start = true;
                                }
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    if self.version_in(&[18]) {
                                        self.emit(
                                            Tok::tLPAREN2,
                                            TokenValue::from(String::from("(").as_str()),
                                            (self.te - 1),
                                            self.te,
                                        );
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    } else {
                                        self.emit(
                                            Tok::tLPAREN_ARG,
                                            TokenValue::from(String::from("(").as_str()),
                                            (self.te - 1),
                                            self.te,
                                        );
                                        self.cs = 345;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                }
                            }
                        }
                        151 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                                if self.version_in(&[18]) {
                                    self.command_start = true;
                                }
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tLPAREN2,
                                        TokenValue::from(String::from("(").as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        159 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                                if self.version_in(&[18]) {
                                    self.command_start = true;
                                }
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tLPAREN_ARG,
                                        TokenValue::from(String::from("(").as_str()),
                                        (self.te - 1),
                                        self.te,
                                    );
                                    if self.version_in(&[18]) {
                                        self.cs = 508;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    } else {
                                        self.cs = 345;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    }
                                }
                            }
                        }
                        208 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                                if self.version_in(&[18]) {
                                    self.command_start = true;
                                }
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::tLPAREN,
                                        TokenValue::from(String::from("(").as_str()),
                                        self.ts,
                                        self.te,
                                    );
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        289 => {
                            {
                                self.cond.push(false);
                                self.cmdarg.push(false);
                                self.paren_nest += 1;
                                if self.version_in(&[18]) {
                                    self.command_start = true;
                                }
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit_table(Table::Punctuation, self.ts, self.te);
                                    self.cs = 345;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        290 => {
                            {
                                self.paren_nest -= 1;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit_rbrace_rparen_rbrack();
                                    if ((self.tok(self.ts, self.te) == String::from("}"))
                                        || (self.tok(self.ts, self.te) == String::from("]")))
                                    {
                                        if (self.version >= 25) {
                                            self.cs = 516;
                                        } else {
                                            self.cs = 313;
                                        }
                                    } else {
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        57 => {
                            {
                                heredoc_e = p;
                            }
                            {
                                self.newline_s = p;
                            }
                        }
                        233 => {
                            {
                                new_herebody_s = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    {
                                        captures = heredoc_captures(&self.tok(self.ts, heredoc_e));
                                        true
                                    };
                                    indent = (!(captures[1].clone().is_empty())
                                        || !(captures[2].clone().is_empty()));
                                    dedent_body = !(captures[2].clone().is_empty());
                                    type_ = (if captures[3].clone().is_empty() {
                                        String::from("<<\"")
                                    } else {
                                        (format!("{}{}", String::from("<<"), captures[3].clone()))
                                    });
                                    delimiter = captures[4].clone();
                                    if (self.version >= 27) {
                                        if (((delimiter
                                            .clone()
                                            .matches(String::from("\n").as_str())
                                            .count()
                                            as i64)
                                            > 0)
                                            || ((delimiter
                                                .clone()
                                                .matches(String::from("\r").as_str())
                                                .count()
                                                as i64)
                                                > 0))
                                        {
                                            self.diagnostic(
                                                Level::Error,
                                                "unterminated heredoc id".to_owned(),
                                                (self.ts, (self.ts + 1)),
                                            );
                                        }
                                    } else if (self.version >= 24) {
                                        if ((delimiter
                                            .clone()
                                            .matches(String::from("\n").as_str())
                                            .count()
                                            as i64)
                                            > 0)
                                        {
                                            if delimiter
                                                .clone()
                                                .ends_with(String::from("\n").as_str())
                                            {
                                                delimiter = delimiter.clone().trim_end().to_owned();
                                            } else {
                                                self.diagnostic(Level::Fatal, "here document identifier across newlines, never match".to_owned(), (self.ts, (self.ts + 1)));
                                                return Err(Fatal);
                                            }
                                        }
                                    }
                                    if (dedent_body && self.version_in(&[18, 19, 20, 21, 22])) {
                                        self.emit(
                                            Tok::tLSHFT,
                                            TokenValue::from(String::from("<<").as_str()),
                                            self.ts,
                                            (self.ts + 2),
                                        );
                                        p = (self.ts + 1);
                                        self.cs = 345;
                                        {
                                            p += 1;
                                            goto_level = L_OUT;
                                            continue 'exec;
                                        }
                                    } else {
                                        self.push_literal(
                                            type_.clone(),
                                            delimiter.clone(),
                                            self.ts,
                                            Some(heredoc_e),
                                            indent,
                                            dedent_body,
                                            false,
                                        );
                                        if self.strings.herebody_s.is_none() {
                                            self.strings.herebody_s = Some(new_herebody_s);
                                        }
                                        p = (self.strings.herebody_s.unwrap_or(NIL) - 1);
                                        self.cs = 128;
                                    }
                                }
                            }
                        }
                        228 => {
                            {
                                tm = (p - 1);
                                diag_msg = "ivar_name";
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit_colon_with_digits(p, tm, diag_msg);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        231 => {
                            {
                                tm = (p - 2);
                                diag_msg = "cvar_name";
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit_colon_with_digits(p, tm, diag_msg);
                                    self.cs = 516;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        241 => {
                            {
                                tm = p;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    self.emit(
                                        Tok::kRESCUE,
                                        TokenValue::from(String::from("rescue").as_str()),
                                        self.ts,
                                        tm,
                                    );
                                    p = (tm - 1);
                                    self.cs = 321;
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        332 => {
                            {
                                self.num_base = 16;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        326 => {
                            {
                                self.num_base = 10;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        329 => {
                            {
                                self.num_base = 8;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        323 => {
                            {
                                self.num_base = 2;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        338 => {
                            {
                                self.num_base = 10;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        301 => {
                            {
                                self.num_base = 8;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        339 => {
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                        }
                        184 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.newline_s = p;
                            }
                        }
                        305 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.num_suffix_s = p;
                            }
                        }
                        107 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 4;
                            }
                        }
                        94 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 5;
                            }
                        }
                        90 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 6;
                            }
                        }
                        12 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 33;
                            }
                        }
                        140 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 34;
                            }
                        }
                        13 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 39;
                            }
                        }
                        133 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 40;
                            }
                        }
                        160 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 46;
                            }
                        }
                        31 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 47;
                            }
                        }
                        181 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 54;
                            }
                        }
                        170 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 55;
                            }
                        }
                        55 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 67;
                            }
                        }
                        246 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 76;
                            }
                        }
                        185 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 80;
                            }
                        }
                        240 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 81;
                            }
                        }
                        239 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 82;
                            }
                        }
                        59 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 86;
                            }
                        }
                        182 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 87;
                            }
                        }
                        186 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 91;
                            }
                        }
                        356 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 104;
                            }
                        }
                        351 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 105;
                            }
                        }
                        359 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 107;
                            }
                        }
                        352 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 108;
                            }
                        }
                        353 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 109;
                            }
                        }
                        358 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 110;
                            }
                        }
                        350 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 111;
                            }
                        }
                        344 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 112;
                            }
                        }
                        270 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 113;
                            }
                        }
                        303 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 116;
                            }
                        }
                        67 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 117;
                            }
                        }
                        273 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 119;
                            }
                        }
                        264 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 123;
                            }
                        }
                        275 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 124;
                            }
                        }
                        268 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 126;
                            }
                        }
                        274 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 127;
                            }
                        }
                        73 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 140;
                            }
                        }
                        363 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.act = 144;
                            }
                        }
                        47 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.newline_s = p;
                            }
                        }
                        100 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        115 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        127 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        149 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    {
                                        self.cs = 516;
                                        goto_level = L_AGAIN;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        164 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        176 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        203 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        251 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        261 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        282 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        377 => {
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                            }
                        }
                        334 => {
                            {
                                self.num_base = 10;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                        }
                        298 => {
                            {
                                self.num_base = 8;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                        }
                        313 => {
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        306 => {
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Float;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        302 => {
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Float;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.tok(self.ts, self.num_suffix_s);
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tFLOAT,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        155 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.newline_s = p;
                            }
                            {
                                self.act = 40;
                            }
                        }
                        21 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.act = 39;
                            }
                        }
                        32 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.act = 47;
                            }
                        }
                        79 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                p = self.on_newline(p);
                            }
                            {
                                self.act = 140;
                            }
                        }
                        51 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.act = 60;
                            }
                        }
                        70 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.act = 106;
                            }
                        }
                        78 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.act = 140;
                            }
                        }
                        23 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                tm = p;
                            }
                            {
                                self.act = 34;
                            }
                        }
                        243 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                tm = p;
                            }
                            {
                                self.act = 86;
                            }
                        }
                        242 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                tm = p;
                            }
                            {
                                self.act = 87;
                            }
                        }
                        335 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.num_base = 10;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.act = 113;
                            }
                        }
                        330 => {
                            {
                                self.num_base = 16;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        324 => {
                            {
                                self.num_base = 10;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        327 => {
                            {
                                self.num_base = 8;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        321 => {
                            {
                                self.num_base = 2;
                                self.num_digits_s = p;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        333 => {
                            {
                                self.num_base = 10;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        297 => {
                            {
                                self.num_base = 8;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.te = p;
                                p = (p - 1);
                                {
                                    digits = self.numeric_literal_int();
                                    if self.version_in(&[18, 19, 20]) {
                                        self.emit(
                                            Tok::tINTEGER,
                                            TokenValue::from(digits.clone().as_str()),
                                            self.ts,
                                            self.num_suffix_s,
                                        );
                                        p = (self.num_suffix_s - 1);
                                    } else {
                                        p = self.num_xfrm_call(digits.clone(), p);
                                    }
                                    {
                                        p += 1;
                                        goto_level = L_OUT;
                                        continue 'exec;
                                    }
                                }
                            }
                        }
                        17 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                p = self.on_newline(p);
                            }
                            {
                                tm = p;
                            }
                            {
                                self.act = 34;
                            }
                        }
                        48 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.act = 60;
                            }
                        }
                        77 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.sharp_s = (p - 1);
                            }
                            {
                                self.emit_comment_from_range(p, pe);
                            }
                            {
                                self.act = 140;
                            }
                        }
                        340 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.act = 115;
                            }
                        }
                        336 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.num_base = 10;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.act = 115;
                            }
                        }
                        300 => {
                            {
                                self.te = (p + 1);
                            }
                            {
                                self.num_base = 8;
                                self.num_digits_s = self.ts;
                            }
                            {
                                self.num_suffix_s = p;
                            }
                            {
                                self.num_xfrm = NumXfrm::Integer;
                            }
                            {
                                self.act = 115;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if goto_level <= L_AGAIN {
                match TO_STATE_ACTIONS[self.cs as usize] {
                    84 => {
                        self.ts = NIL;
                    }
                    _ => {}
                }
                if self.cs == 0 {
                    goto_level = L_OUT;
                    continue 'exec;
                }
                p += 1;
                if p != pe {
                    goto_level = L_RESUME;
                    continue 'exec;
                }
            }
            if goto_level <= L_TEST_EOF && p == eof && EOF_TRANS[self.cs as usize] > 0 {
                trans = usize::from(EOF_TRANS[self.cs as usize]) - 1;
                goto_level = L_EOF_TRANS;
                continue 'exec;
            }
            break 'exec;
        }
        Ok(p)
    }
}
