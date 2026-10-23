//! racc 1.8.1 の C 実装 (`ext/racc/cparse/cparse.c` の `parse_main` と `reduce0`) を移したもの。
//!
//! RuboCop は MRI では `all_errors_are_fatal = false` で parser gem を動かすので、構文エラーの後も
//! racc の誤り回復で読み続け、診断を集める。どの診断が出てどれが出ないかは回復の手順そのもので
//! 決まるため、制御の順序 (既定の還元、次のトークンを読む時機、`errstatus` の増減、`error` の
//! shift と pop) を cparse のとおりに保つ。Ruby 実装の racc (`_racc_do_parse_rb`) は `error` の
//! shift でも `errstatus` を減らして現トークンを消費するので、結果が cparse と違う。

use std::collections::HashSet;
use std::rc::Rc;

use super::actions;
use super::lexer::Lexer;
use super::tables::actions::Action;
use super::tables::tokens::Tok;
use super::tables::{Grammar, entry};
use super::value::{Context, Span, Token, Value};
use super::{Diagnostic, Level, Version};

/// 文法動作や builder が `yyerror` を呼んだこと。cparse は還元を途中で止めて誤り回復に入る。
#[derive(Debug)]
pub(crate) struct YyError;

/// 字句解析器の fatal 診断。parser gem は `Parser::SyntaxError` を投げ、RuboCop はそこで解析を打ち切る。
#[derive(Debug)]
pub(crate) struct Fatal;

pub(crate) type ActionResult<T> = Result<T, YyError>;

/// `Parser::MaxNumparamStack`。
#[derive(Default)]
pub(crate) struct MaxNumparamStack {
    pub(crate) stack: Vec<(i32, bool)>,
}

impl MaxNumparamStack {
    const ORDINARY_PARAMS: i32 = -1;

    pub(crate) fn has_ordinary_params(&mut self) {
        self.set(Self::ORDINARY_PARAMS);
    }

    pub(crate) fn is_ordinary_params(&self) -> bool {
        self.top() == Some(Self::ORDINARY_PARAMS)
    }

    pub(crate) fn has_numparams(&self) -> bool {
        self.top().is_some_and(|top| top > 0)
    }

    pub(crate) fn register(&mut self, numparam: i32) {
        let top = self.top().unwrap_or(0);
        self.set(top.max(numparam));
    }

    pub(crate) fn top(&self) -> Option<i32> {
        self.stack.last().map(|&(value, _)| value)
    }

    pub(crate) fn push(&mut self, is_static: bool) {
        self.stack.push((0, is_static));
    }

    pub(crate) fn pop(&mut self) -> Option<i32> {
        self.stack.pop().map(|(value, _)| value)
    }

    fn set(&mut self, value: i32) {
        // Ruby では空のスタックへの代入は `nil[:value] =` で例外になるが、文法がそれを起こさない。
        if let Some(last) = self.stack.last_mut() {
            last.0 = value;
        }
    }
}

/// `Parser::CurrentArgStack` の要素 (`nil`・引数名・`0`)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CurrentArg {
    Nil,
    Zero,
    Name(Rc<str>),
}

/// `Parser::VariablesStack`。
pub(crate) struct VariablesStack {
    stack: Vec<HashSet<Rc<str>>>,
}

impl VariablesStack {
    fn new() -> Self {
        Self {
            stack: vec![HashSet::new()],
        }
    }

    /// `Base#reset` は `@stack.clear` を呼ぶので、最初の `push` 前の要素は無い。
    fn reset(&mut self) {
        self.stack.clear();
    }

    pub(crate) fn push(&mut self) {
        self.stack.push(HashSet::new());
    }

    pub(crate) fn pop(&mut self) {
        self.stack.pop();
    }

    pub(crate) fn declare(&mut self, name: &str) {
        if let Some(last) = self.stack.last_mut() {
            last.insert(Rc::from(name));
        }
    }

    pub(crate) fn is_declared(&self, name: &str) -> bool {
        self.stack.last().is_some_and(|last| last.contains(name))
    }
}

pub(crate) struct Parser {
    pub(crate) grammar: &'static Grammar,
    pub(crate) version: Version,
    pub(crate) lexer: Lexer,
    state: Vec<i16>,
    vstack: Vec<Value>,
    curstate: i16,
    pub(crate) errstatus: u8,
    pub(crate) max_numparam_stack: MaxNumparamStack,
    pub(crate) current_arg_stack: Vec<CurrentArg>,
    pub(crate) pattern_variables: VariablesStack,
    pub(crate) pattern_hash_keys: VariablesStack,
    /// `@last_token`: 最後に字句解析器から受け取ったトークン。`command_args` の動作が先読みを見る。
    pub(crate) last_token: Option<Rc<Token>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// cparse の `handle_act` 以降の分岐。
enum Step {
    Shift(i16),
    Reduce(usize),
    Error,
    UserYyerror,
    Accept,
}

impl Parser {
    pub(crate) fn new(grammar: &'static Grammar, version: Version, lexer: Lexer) -> Self {
        let mut pattern_variables = VariablesStack::new();
        let mut pattern_hash_keys = VariablesStack::new();
        pattern_variables.reset();
        pattern_hash_keys.reset();
        Self {
            grammar,
            version,
            lexer,
            state: Vec::new(),
            vstack: Vec::new(),
            curstate: 0,
            errstatus: 0,
            max_numparam_stack: MaxNumparamStack::default(),
            current_arg_stack: Vec::new(),
            pattern_variables,
            pattern_hash_keys,
            last_token: None,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn context(&self) -> &Context {
        &self.lexer.context
    }

    pub(crate) fn context_mut(&mut self) -> &mut Context {
        &mut self.lexer.context
    }

    fn step_for(&self, act: i16) -> Step {
        let g = self.grammar;
        if act > 0 && act < g.shift_n {
            Step::Shift(act)
        } else if act < 0 && act > -g.reduce_n {
            Step::Reduce(usize::from(act.unsigned_abs()))
        } else if act == -g.reduce_n {
            Step::Error
        } else if act == g.shift_n {
            Step::Accept
        } else {
            unreachable!("racc の表に無い動作 {act}")
        }
    }

    /// `do_parse`。fatal 診断で打ち切ったときは `Err` を返す (診断は記録済み)。
    pub(crate) fn run(&mut self) -> Result<(), Fatal> {
        let g = self.grammar;
        let mut read_next = true;
        // cparse は最初の `t` を `$end` 以外 (FINAL_TOKEN + 1) にしておく。
        let mut t: i32 = 1;
        let mut val = Value::Nil;
        self.state = vec![0];
        self.vstack = Vec::new();
        self.curstate = 0;
        self.errstatus = 0;
        loop {
            let act = match entry(g.action_pointer, i32::from(self.curstate)) {
                None => g.action_default[self.curstate as usize],
                Some(pointer) => {
                    if read_next && t != 0 {
                        let token = self.next_token()?;
                        t = if token.tok == Tok::EOF {
                            0
                        } else {
                            i32::from(g.token_ids[token.tok as usize])
                        };
                        val = Value::Token(token);
                    }
                    read_next = false;
                    let index = i32::from(pointer) + t;
                    match entry(g.action_table, index) {
                        Some(act) if entry(g.action_check, index) == Some(self.curstate) => act,
                        _ => g.action_default[self.curstate as usize],
                    }
                }
            };
            let mut step = self.step_for(act);
            loop {
                match step {
                    Step::Shift(next) => {
                        if self.errstatus > 0 {
                            self.errstatus -= 1;
                        }
                        self.shift(next, val.clone());
                        read_next = true;
                        break;
                    }
                    Step::Reduce(rule) => match self.reduce(rule) {
                        Ok(()) => break,
                        Err(YyError) => step = Step::UserYyerror,
                    },
                    Step::Error => {
                        if self.errstatus == 0 {
                            self.on_error(t, &val);
                        }
                        step = Step::UserYyerror;
                    }
                    Step::UserYyerror => {
                        if self.errstatus == 3 {
                            if t == 0 {
                                // CP_FIN_EOT: 回復の直後にまた EOF で行き詰まった。
                                return Ok(());
                            }
                            read_next = true;
                        }
                        self.errstatus = 3;
                        let error_act = loop {
                            if let Some(act) = self.error_action() {
                                break act;
                            }
                            if self.state.len() <= 1 {
                                // CP_FIN_CANTPOP
                                return Ok(());
                            }
                            self.state.pop();
                            self.vstack.pop();
                            self.curstate = *self.state.last().expect("state stack");
                        };
                        match self.step_for(error_act) {
                            // `error` の shift は errstatus を減らさず、現トークンも読み進めない。
                            Step::Shift(next) => {
                                self.shift(next, val.clone());
                                break;
                            }
                            Step::Reduce(rule) => step = Step::Reduce(rule),
                            Step::Accept => return Ok(()),
                            Step::Error | Step::UserYyerror => {
                                unreachable!("error トークンの動作が誤り")
                            }
                        }
                    }
                    Step::Accept => return Ok(()),
                }
            }
        }
    }

    /// 現在の状態で `error` トークンを扱える動作。
    fn error_action(&self) -> Option<i16> {
        let g = self.grammar;
        let pointer = entry(g.action_pointer, i32::from(self.curstate))?;
        let index = i32::from(pointer) + 1;
        let act = entry(g.action_table, index)?;
        (entry(g.action_check, index) == Some(self.curstate)).then_some(act)
    }

    fn shift(&mut self, next: i16, value: Value) {
        self.vstack.push(value);
        self.curstate = next;
        self.state.push(next);
    }

    fn reduce(&mut self, rule_no: usize) -> Result<(), YyError> {
        let g = self.grammar;
        let rule = &g.rules[rule_no];
        let len = usize::from(rule.len);
        let (val, mut result) = if len == 0 {
            (Vec::new(), Value::Nil)
        } else {
            let at = self.vstack.len() - len;
            let val = self.vstack.split_off(at);
            self.state.truncate(self.state.len() - len);
            let first = if actions::list_operation(rule.action).is_some() {
                Value::Nil
            } else {
                val[0].clone()
            };
            (val, first)
        };
        if rule.action != Action::None {
            // yyerror なら、cparse と同じく切り詰めたスタックのまま還元を中断する。
            result = actions::run(self, rule.action, val, result)?;
        }
        self.vstack.push(result);
        let k2 = *self.state.last().expect("state stack");
        let k1 = rule.lhs as i16 - g.nt_base;
        let goto_state = entry(g.goto_pointer, i32::from(k1))
            .map(|pointer| i32::from(pointer) + i32::from(k2))
            .and_then(|index| {
                let next = entry(g.goto_table, index)?;
                (entry(g.goto_check, index) == Some(k1)).then_some(next)
            })
            .unwrap_or_else(|| g.goto_default[k1 as usize]);
        self.state.push(goto_state);
        self.curstate = goto_state;
        Ok(())
    }

    fn next_token(&mut self) -> Result<Rc<Token>, Fatal> {
        let token = self.lexer.advance();
        // 字句解析器の診断は、その字句を構文解析器が受け取る前に起きている。
        self.diagnostics.append(&mut self.lexer.diagnostics);
        let token = Rc::new(token?);
        self.last_token = Some(Rc::clone(&token));
        Ok(token)
    }

    /// `Parser::Base#on_error`。
    fn on_error(&mut self, t: i32, val: &Value) {
        let name = self.grammar.names[t as usize];
        let range = val.as_token().map_or(0..0, |token| token.range.clone());
        self.diagnostics.push(Diagnostic {
            level: Level::Error,
            message: format!("unexpected token {name}"),
            range,
        });
    }

    /// `Parser::Base#diagnostic`: error なら `yyerror` で回復に入る。
    pub(crate) fn error(&mut self, message: String, range: Option<Span>) -> ActionResult<()> {
        self.diagnostics.push(Diagnostic {
            level: Level::Error,
            message,
            range: range.unwrap_or(0..0),
        });
        Err(YyError)
    }

    /// `yyerrok`。
    pub(crate) fn yyerrok(&mut self) {
        self.errstatus = 0;
    }

    /// `local_push`。
    pub(crate) fn local_push(&mut self) {
        self.lexer.static_env.extend_static();
        self.lexer.cmdarg.push(false);
        self.lexer.cond.push(false);
        self.max_numparam_stack.push(true);
    }

    /// `local_pop`。
    pub(crate) fn local_pop(&mut self) {
        self.lexer.static_env.unextend();
        self.lexer.cmdarg.pop();
        self.lexer.cond.pop();
        self.max_numparam_stack.pop();
    }

    pub(crate) fn last_token_is(&self, tok: Tok) -> bool {
        self.last_token
            .as_ref()
            .is_some_and(|token| token.tok == tok)
    }

    pub(crate) fn current_arg_set(&mut self, value: CurrentArg) {
        if let Some(last) = self.current_arg_stack.last_mut() {
            *last = value;
        } else {
            // `@stack[@stack.length - 1] = value` は空のときに添字 -1 へ代入しようとして例外になる。
            // 文法が空のまま呼ぶことは無い。
            self.current_arg_stack.push(value);
        }
    }
}
