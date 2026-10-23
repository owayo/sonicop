//! parser gem の文法動作 (`ruby27.y` / `ruby30.y` / `ruby31.y` / `ruby32.y` の `{ ... }`) を移したもの。
//!
//! 列挙子の名前は動作本文の SHA-256 の先頭 8 桁で、本文は `tables/actions.rs` の doc にある。
//! gem の更新で本文が変わると列挙子が変わり、この照合が網羅的でなくなってコンパイルが止まる。
//! 各腕は Ruby の本文と同じ順序で副作用 (字句解析器の状態・静的環境・文脈) を起こし、`yyerror` を
//! 呼ぶ診断の後は何もしない (`?` で中断する)。埋め込み動作 (長さ 0 の規則) の `val` は cparse が
//! 空配列を渡すので、`val[1]` などは `nil` になる。

use std::rc::Rc;

use super::lexer::LexState;
use super::parser::{ActionResult, CurrentArg, Parser};
use super::tables::actions::Action;
use super::tables::tokens::Tok;
use super::value::{Token, TokenValue, Value};

fn g(val: &[Value], index: usize) -> Value {
    val.get(index).cloned().unwrap_or_default()
}

fn list(items: Vec<Value>) -> Value {
    Value::List(items)
}

/// `[ a, *b, c ]` の組み立て。
fn splat(value: Value) -> Vec<Value> {
    value.into_list()
}

/// `a << b` / `a.push(b)`。
fn push(target: Value, item: Value) -> Value {
    let mut items = target.into_list();
    items.push(item);
    list(items)
}

/// `a.concat(b)`。
fn concat(target: Value, other: Value) -> Value {
    let mut items = target.into_list();
    items.extend(other.into_list());
    list(items)
}

/// トークンの値 (`val[i][0]`)。
fn name(value: &Value) -> String {
    value.token_str().to_owned()
}

/// `[:dot, val[0][1]]`: 値が記号の疑似トークン。
fn call_op(value: &Value, op: &'static str) -> Value {
    let range = value.as_token().map_or(0..0, |token| token.range.clone());
    Value::Token(Rc::new(Token {
        tok: Tok::tDOT,
        value: TokenValue::Sym(op),
        range,
    }))
}

impl Parser {
    /// Base#diagnostic(:error, ..., location_t): トークンの位置で診断して回復に入る。
    fn grammar_error(&mut self, message: &str, location: &Value) -> ActionResult<()> {
        self.error(
            message.to_owned(),
            location.as_token().map(|token| token.range.clone()),
        )
    }

    fn set_in_kwarg(&mut self, value: &Value) {
        self.context_mut().in_kwarg = value.truthy();
    }

    fn numargs_or(&mut self, args: Value) -> Value {
        if self.max_numparam_stack.has_numparams() {
            let top = self.max_numparam_stack.top().unwrap_or(0);
            self.b_numargs(top)
        } else {
            args
        }
    }

    /// `endless_method_name(name_t)` (3.0 以降の inner 節)。
    fn endless_method_name(&mut self, name_t: &Value) -> ActionResult<()> {
        let name = name_t.token_str();
        if !matches!(name, "===" | "==" | "!=" | "<=" | ">=") && name.ends_with('=') {
            self.grammar_error(
                "setter method cannot be defined in an endless method definition",
                name_t,
            )?;
        }
        Ok(())
    }

    /// `check_kwarg_name(name_t)`。
    fn check_kwarg_name(&mut self, name_t: &Value) -> ActionResult<()> {
        if name_t
            .token_str()
            .starts_with(|c: char| c.is_ascii_uppercase())
        {
            self.grammar_error("formal argument cannot be a constant", name_t)?;
        }
        Ok(())
    }

    fn pop_current_arg(&mut self) {
        self.current_arg_stack.pop();
    }

    /// `*bodies, (else_t, else_body) = *val`。
    fn split_else(value: Value) -> (Vec<Value>, Value, Value) {
        let mut items = value.into_list();
        let last = items.pop().unwrap_or_default();
        (items, last.at(0), last.at(1))
    }
}

/// 配列を足すだけの還元は元の値を消費し、巨大な Ruby の配列で各要素ごとの全コピーを避ける。
pub(super) fn list_operation(action: Action) -> Option<(usize, usize, bool)> {
    match action {
        Action::Af55455b1 => Some((0, 2, false)),
        Action::A7c3b3cb3 => Some((0, 1, false)),
        Action::Ad4231615 => Some((0, 1, false)),
        Action::Abbd434f2 => Some((0, 3, false)),
        Action::A2747f67b => Some((0, 1, true)),
        Action::A2c0631bd => Some((0, 2, false)),
        Action::Abc102fa6 => Some((0, 1, true)),
        Action::A7f1063c7 => Some((0, 1, false)),
        Action::A8dfcf5c0 => Some((0, 2, false)),
        Action::A804b401c => Some((0, 2, true)),
        Action::A6caa26d5 => Some((0, 2, true)),
        _ => None,
    }
}

pub(crate) fn run(
    p: &mut Parser,
    action: Action,
    mut val: Vec<Value>,
    result: Value,
) -> ActionResult<Value> {
    if let Some((left, right, concatenate)) = list_operation(action) {
        let left = std::mem::take(&mut val[left]);
        let right = std::mem::take(&mut val[right]);
        return Ok(if concatenate {
            concat(left, right)
        } else {
            push(left, right)
        });
    }
    let v = |index: usize| g(&val, index);
    Ok(match action {
        Action::None => result,
        Action::Af97c9d62 => {
            p.current_arg_stack.push(CurrentArg::Nil);
            p.max_numparam_stack.push(false);
            result
        }
        Action::Aa07db756 => {
            let result = v(1);
            p.pop_current_arg();
            p.max_numparam_stack.pop();
            result
        }
        Action::A303c0362 => p.b_compstmt(v(0)),
        Action::A1142a778 => list(Vec::new()),
        Action::Aec2d86e3 => list(vec![v(0)]),
        Action::Af55455b1 => push(v(0), v(2)),
        Action::A88e3ed53 => list(vec![v(1)]),
        Action::A7bb825c5 => {
            let block = v(1);
            p.b_prepost_exe("preexe", v(0), block.at(1), block.at(2))
        }
        Action::Ad02d1a27 => list(val.clone()),
        Action::A927a96c0 => {
            let rescue_bodies = v(1);
            let else_ = v(2);
            let ensure = v(3);
            if rescue_bodies.as_list().is_empty() && !else_.at(0).is_nil() {
                p.grammar_error("else without rescue is useless", &else_.at(0))?;
            }
            p.b_begin_body(
                v(0),
                rescue_bodies.into_list(),
                else_.at(0),
                else_.at(1),
                ensure.at(0),
                ensure.at(1),
            )
        }
        Action::A1405bbe5 => {
            p.grammar_error("BEGIN in method", &v(0))?;
            result
        }
        Action::A589c228e => {
            p.lexer.set_state(LexState::ExprFname);
            result
        }
        Action::A6a3141e7 => p.b_alias(v(0), v(1), v(3)),
        Action::Ae0eac606 => {
            let to = p.b_gvar(v(1))?;
            let from = p.b_gvar(v(2))?;
            p.b_alias(v(0), to, from)
        }
        Action::Ae398a702 => {
            let to = p.b_gvar(v(1))?;
            let from = p.b_back_ref(v(2));
            p.b_alias(v(0), to, from)
        }
        Action::Aaa5cbd81 => {
            p.grammar_error(
                "cannot define an alias for a back-reference variable",
                &v(2),
            )?;
            result
        }
        Action::A4bbff142 => p.b_undef_method(v(0), v(1)),
        Action::A644653a1 => p.b_condition_mod(v(0), Value::Nil, v(2)),
        Action::A59f79666 => p.b_condition_mod(Value::Nil, v(0), v(2)),
        Action::A05f9f431 => p.b_loop_mod("while", v(0), v(2)),
        Action::A5dc3c211 => p.b_loop_mod("until", v(0), v(2)),
        Action::A9865da41 => {
            let rescue_body = p.b_rescue_body(v(1), Value::Nil, Value::Nil, Value::Nil, v(2));
            p.b_begin_body(
                v(0),
                vec![rescue_body],
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Nil,
            )
        }
        Action::A23524b49 => p.b_prepost_exe("postexe", v(0), v(2), v(3)),
        Action::A3073dff7 => p.b_multi_assign(v(0), v(2)),
        Action::Af0ef13f6 => {
            let rhs = p.b_array(Value::Nil, v(2), Value::Nil);
            p.b_assign(v(0), rhs)
        }
        Action::A46561a17 => {
            let rescue_body = p.b_rescue_body(v(3), Value::Nil, Value::Nil, Value::Nil, v(4));
            let begin_body = p.b_begin_body(
                v(2),
                vec![rescue_body],
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Nil,
            );
            p.b_multi_assign(v(0), begin_body)
        }
        Action::A339b1bf4 => p.b_assign(v(0), v(2)),
        Action::A56d07ccc => p.b_op_assign(v(0), v(1), v(2))?,
        Action::A7654d351 => {
            let index = p.b_index(v(0), v(3), v(2), false);
            p.b_op_assign(index, v(4), v(5))?
        }
        Action::A11286b4d => {
            let call = p.b_call_method(v(0), v(1), v(2), Value::Nil, list(Vec::new()), Value::Nil);
            p.b_op_assign(call, v(3), v(4))?
        }
        Action::Ab6eab20c => {
            let fetched = p.b_const_fetch(v(0), v(2));
            let constant = p.b_const_op_assignable(fetched);
            p.b_op_assign(constant, v(3), v(4))?
        }
        Action::Acde2b4f8 => {
            p.b_op_assign(v(0), v(1), v(2))?;
            result
        }
        Action::A073a7e79 => p.b_logical_op("and", v(0), v(2)),
        Action::A697f828e => p.b_logical_op("or", v(0), v(2)),
        Action::Abf073d81 => p.b_not_op(v(0), Value::Nil, v(2), Value::Nil),
        Action::Aacb86ab1 => p.b_not_op(v(0), Value::Nil, v(1), Value::Nil),
        Action::A356620a4 => {
            p.lexer.set_state(LexState::ExprBeg);
            p.lexer.command_start = false;
            p.pattern_variables.push();
            let result = Value::Bool(p.context().in_kwarg);
            p.context_mut().in_kwarg = true;
            result
        }
        Action::Ae166e7bf => {
            p.pattern_variables.pop();
            p.set_in_kwarg(&v(2));
            // RuboCop の builder は emit_match_pattern を立てる。
            p.b_match_binary("match_pattern", v(0), v(3))
        }
        Action::Accd7b4c6 => {
            p.lexer.cond.push(true);
            result
        }
        Action::A77d3fc34 => {
            p.lexer.cond.pop();
            list(vec![v(1), v(2)])
        }
        Action::A8afa606e => p.b_call_method(v(0), v(1), v(2), Value::Nil, v(3), Value::Nil),
        Action::Aff9a11eb => {
            let result = Value::Context(*p.context());
            p.context_mut().in_block = true;
            result
        }
        Action::Ada877101 => {
            let mut items = vec![v(0)];
            items.extend(splat(v(2)));
            items.push(v(3));
            p.context_mut().in_block = v(1).as_context().in_block;
            list(items)
        }
        Action::A92f71b83 => {
            p.b_call_method(Value::Nil, Value::Nil, v(0), Value::Nil, v(1), Value::Nil)
        }
        Action::A56df17a3 => {
            let call = p.b_call_method(Value::Nil, Value::Nil, v(0), Value::Nil, v(1), Value::Nil);
            let block = v(2);
            p.b_block(call, block.at(0), block.at(1), block.at(2), block.at(3))?
        }
        Action::Aa908316f => {
            let call = p.b_call_method(v(0), v(1), v(2), Value::Nil, v(3), Value::Nil);
            let block = v(4);
            p.b_block(call, block.at(0), block.at(1), block.at(2), block.at(3))?
        }
        Action::Acf42b7e1 => p.b_keyword_cmd("super", v(0), Value::Nil, v(1), Value::Nil)?,
        Action::A49880833 => p.b_keyword_cmd("yield", v(0), Value::Nil, v(1), Value::Nil)?,
        Action::A6007f1a1 => p.b_keyword_cmd("return", v(0), Value::Nil, v(1), Value::Nil)?,
        Action::Ab818b18a => p.b_keyword_cmd("break", v(0), Value::Nil, v(1), Value::Nil)?,
        Action::Ad6aa70f3 => p.b_keyword_cmd("next", v(0), Value::Nil, v(1), Value::Nil)?,
        Action::A9ed60617 => p.b_multi_lhs(Value::Nil, v(0), Value::Nil),
        Action::Accdcd3c7 => p.b_begin(v(0), v(1), v(2)),
        Action::Ae66ee726 => p.b_multi_lhs(v(0), v(1), v(2)),
        Action::A7c3b3cb3 => push(v(0), v(1)),
        Action::A2e8e96f1 => {
            let item = p.b_splat(v(1), v(2));
            push(v(0), item)
        }
        Action::A3881d925 => {
            let item = p.b_splat(v(1), v(2));
            concat(push(v(0), item), v(4))
        }
        Action::A3ec5887f => {
            let item = p.b_splat(v(1), Value::Nil);
            push(v(0), item)
        }
        Action::Aef2a3ac4 => {
            let item = p.b_splat(v(1), Value::Nil);
            concat(push(v(0), item), v(3))
        }
        Action::A00a4a5cc => list(vec![p.b_splat(v(0), v(1))]),
        Action::Ade3a9256 => {
            let mut items = vec![p.b_splat(v(0), v(1))];
            items.extend(splat(v(3)));
            list(items)
        }
        Action::A34be6a6c => list(vec![p.b_splat(v(0), Value::Nil)]),
        Action::Ae50dd124 => {
            let mut items = vec![p.b_splat(v(0), Value::Nil)];
            items.extend(splat(v(2)));
            list(items)
        }
        Action::Ad4231615 => push(v(0), v(1)),
        Action::A3119d3f8 => p.b_assignable(v(0))?,
        Action::A2e3b88ae => p.b_index(v(0), v(3), v(2), true),
        Action::Ade28d5df => {
            if matches!(
                v(1).as_token().map(|token| &token.value),
                Some(TokenValue::Sym("anddot"))
            ) {
                p.grammar_error("&. inside multiple assignment destination", &v(1))?;
            }
            p.b_attr_asgn(v(0), v(1), v(2))
        }
        Action::A952c1f18 => p.b_attr_asgn(v(0), v(1), v(2)),
        Action::Ae6a95782 => {
            let fetched = p.b_const_fetch(v(0), v(2));
            p.b_assignable(fetched)?
        }
        Action::Aa2eaed0a => {
            let global = p.b_const_global(v(0), v(1));
            p.b_assignable(global)?
        }
        Action::A90a5256a => {
            p.grammar_error("class or module name must be a constant literal", &v(0))?;
            result
        }
        Action::Ab55a6aa2 => p.b_const_global(v(0), v(1)),
        Action::Ab7c8fc06 => p.b_const(v(0)),
        Action::A367af191 => p.b_const_fetch(v(0), v(2)),
        Action::A4ffc260f => p.b_symbol_internal(v(0))?,
        Action::Abbd434f2 => push(v(0), v(3)),
        Action::A2e05b122 => {
            let global = p.b_const_global(v(0), v(1));
            let constant = p.b_const_op_assignable(global);
            p.b_op_assign(constant, v(2), v(3))?
        }
        Action::A07650c53 => p.b_range("irange", v(0), v(1), v(2)),
        Action::A5cfd954b => p.b_range("erange", v(0), v(1), v(2)),
        Action::A17ef1889 => p.b_range("irange", v(0), v(1), Value::Nil),
        Action::Af7bda890 => p.b_range("erange", v(0), v(1), Value::Nil),
        Action::A59476aca => p.b_range("irange", Value::Nil, v(0), v(1)),
        Action::Aa46e321a => p.b_range("erange", Value::Nil, v(0), v(1)),
        Action::Ae9106b4e => p.b_binary_op(v(0), v(1), v(2)),
        Action::Abd6baefa => {
            let power = p.b_binary_op(v(1), v(2), v(3));
            p.b_unary_op(v(0), power)
        }
        Action::A00b97ef8 => p.b_unary_op(v(0), v(1)),
        Action::Ac5931e06 => p.b_match_op(v(0), v(2)),
        Action::A3f241b5e => {
            p.b_keyword_cmd("defined?", v(0), Value::Nil, list(vec![v(2)]), Value::Nil)?
        }
        Action::A67d438c7 => p.b_ternary(v(0), v(2), v(5)),
        Action::Af6484942 => {
            let hash = p.b_associate(Value::Nil, v(2), Value::Nil);
            push(v(0), hash)
        }
        Action::Aa47e4446 => list(vec![p.b_associate(Value::Nil, v(0), Value::Nil)]),
        Action::A2d1d5e63 => {
            if !p.lexer.static_env.declared_forward_args() {
                p.grammar_error("unexpected token tBDOT3", &v(3))?;
            }
            let mut args = splat(v(1));
            args.push(p.b_forwarded("forwarded_args", v(3)));
            list(vec![v(0), list(args), v(4)])
        }
        Action::A9819c4f1 => {
            if !p.lexer.static_env.declared_forward_args() {
                p.grammar_error("unexpected token tBDOT3", &v(1))?;
            }
            let args = vec![p.b_forwarded("forwarded_args", v(1))];
            list(vec![v(0), list(args), v(2)])
        }
        Action::A2752b003 => list(vec![Value::Nil, list(Vec::new()), Value::Nil]),
        Action::A2747f67b => concat(v(0), v(1)),
        Action::A7b25f037 => {
            let hash = p.b_associate(Value::Nil, v(0), Value::Nil);
            concat(list(vec![hash]), v(1))
        }
        Action::Aa0f2ecf7 => {
            let hash = p.b_associate(Value::Nil, v(2), Value::Nil);
            concat(push(v(0), hash), v(3))
        }
        Action::Ad26d5e74 => list(vec![v(0)]),
        Action::A7143a6f0 => {
            // command_args の前: 先読みで字句解析器が積んだ `[` / `(` の 0 より下へ 1 を入れる。
            if p.last_token_is(Tok::tLBRACK) || p.last_token_is(Tok::tLPAREN_ARG) {
                let top = p.lexer.cmdarg.pop();
                p.lexer.cmdarg.push(true);
                p.lexer.cmdarg.push(top);
            } else {
                p.lexer.cmdarg.push(true);
            }
            result
        }
        Action::A4b3fea33 => {
            if p.last_token_is(Tok::tLBRACE_ARG) {
                let top = p.lexer.cmdarg.pop();
                p.lexer.cmdarg.pop();
                p.lexer.cmdarg.push(top);
            } else {
                p.lexer.cmdarg.pop();
            }
            v(1)
        }
        Action::A7462e75e => p.b_block_pass(v(0), v(1)),
        Action::Ac7a9f196 => {
            let item = p.b_splat(v(2), v(3));
            push(v(0), item)
        }
        Action::A84e4c06e => p.b_array(Value::Nil, v(0), Value::Nil),
        Action::A6529c845 => p.b_call_method(
            Value::Nil,
            Value::Nil,
            v(0),
            Value::Nil,
            list(Vec::new()),
            Value::Nil,
        ),
        Action::A5dc3e21b => {
            p.lexer.cmdarg.push(false);
            result
        }
        Action::Ab44e54e1 => {
            p.lexer.cmdarg.pop();
            p.b_begin_keyword(v(0), v(2), v(3))
        }
        Action::Af4ea540c => {
            p.lexer.set_state(LexState::ExprEndarg);
            result
        }
        Action::A9aa9e915 => p.b_begin(v(0), v(1), v(3)),
        Action::A678b56e6 => p.b_begin(v(0), Value::Nil, v(3)),
        Action::Ad9abff46 => p.b_array(v(0), v(1), v(2)),
        Action::A733530b7 => p.b_associate(v(0), v(1), v(2)),
        Action::A690f7921 => {
            p.b_keyword_cmd("return", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::Abf57b483 => p.b_keyword_cmd("yield", v(0), v(1), v(2), v(3))?,
        Action::A18e81f7e => p.b_keyword_cmd("yield", v(0), v(1), list(Vec::new()), v(2))?,
        Action::Ac0c08ce5 => {
            p.b_keyword_cmd("yield", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::A8e7d7d50 => p.b_keyword_cmd("defined?", v(0), v(2), list(vec![v(3)]), v(4))?,
        Action::A64f508c4 => p.b_not_op(v(0), v(1), v(2), v(3)),
        Action::Af0caa16d => p.b_not_op(v(0), v(1), Value::Nil, v(2)),
        Action::Ab149ce13 => {
            let call = p.b_call_method(
                Value::Nil,
                Value::Nil,
                v(0),
                Value::Nil,
                list(Vec::new()),
                Value::Nil,
            );
            let block = v(1);
            p.b_block(call, block.at(0), block.at(1), block.at(2), block.at(3))?
        }
        Action::A4c29d9cc => {
            let block = v(1);
            p.b_block(v(0), block.at(0), block.at(1), block.at(2), block.at(3))?
        }
        Action::Acfb4c3e6 => {
            let result = Value::Context(*p.context());
            p.context_mut().in_lambda = true;
            result
        }
        Action::Abdf4b1f0 => {
            let lambda_call = p.b_call_lambda(v(0));
            let lambda = v(2);
            let args = lambda.at(0);
            let body = lambda.at(1);
            let result = p.b_block(lambda_call, body.at(0), args, body.at(1), body.at(2))?;
            p.context_mut().in_lambda = v(1).as_context().in_lambda;
            result
        }
        Action::Aea41f5d0 => {
            let else_ = v(4);
            p.b_condition(v(0), v(1), v(2), v(3), else_.at(0), else_.at(1), v(5))
        }
        Action::A0bc894ba => {
            let else_ = v(4);
            p.b_condition(v(0), v(1), v(2), else_.at(1), else_.at(0), v(3), v(5))
        }
        Action::A299bd0ad => {
            let cond = v(1);
            p.b_loop("while", v(0), cond.at(0), cond.at(1), v(2), v(3))
        }
        Action::Aec5b9a1a => {
            let cond = v(1);
            p.b_loop("until", v(0), cond.at(0), cond.at(1), v(2), v(3))
        }
        Action::A45a75655 => {
            let (bodies, else_t, else_body) = Parser::split_else(v(3));
            p.b_case("case", v(0), v(1), bodies, else_t, else_body, v(4))
        }
        Action::A2ad94566 => {
            let (bodies, else_t, else_body) = Parser::split_else(v(2));
            p.b_case("case", v(0), Value::Nil, bodies, else_t, else_body, v(3))
        }
        Action::A2dc3cb3c => {
            let (bodies, else_t, else_body) = Parser::split_else(v(3));
            p.b_case("case_match", v(0), v(1), bodies, else_t, else_body, v(4))
        }
        Action::Ae41e2bcc => {
            let iteratee = v(3);
            p.b_for(v(0), v(1), iteratee.at(0), v(4), v(5))
        }
        Action::A7ad4b3e1 => {
            p.local_push();
            p.context_mut().in_class = true;
            result
        }
        Action::A6d06cc4e => {
            let k_class = v(0);
            let ctx = k_class.at(1).as_context();
            if p.context().in_def {
                p.grammar_error("class definition in method body", &k_class.at(0))?;
            }
            let superclass = v(2);
            let result = p.b_def_class(k_class.at(0), v(1), superclass.at(1), v(4), v(5));
            p.local_pop();
            p.context_mut().in_class = ctx.in_class;
            result
        }
        Action::Aaeaf9e48 => {
            p.context_mut().in_def = false;
            p.context_mut().in_class = false;
            p.local_push();
            result
        }
        Action::Abf2cb3ce => {
            let k_class = v(0);
            let ctx = k_class.at(1).as_context();
            let result = p.b_def_sclass(k_class.at(0), v(2), v(5), v(6));
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.context_mut().in_class = ctx.in_class;
            result
        }
        Action::Aad6896ab => {
            p.context_mut().in_class = true;
            p.local_push();
            result
        }
        Action::A9409bb42 => {
            let k_mod = v(0);
            let ctx = k_mod.at(1).as_context();
            if p.context().in_def {
                p.grammar_error("module definition in method body", &k_mod.at(0))?;
            }
            let result = p.b_def_module(k_mod.at(0), v(1), v(3), v(4));
            p.local_pop();
            p.context_mut().in_class = ctx.in_class;
            result
        }
        Action::A13dfe951 => {
            p.local_push();
            let result = Value::Context(*p.context());
            p.context_mut().in_def = true;
            p.current_arg_stack.push(CurrentArg::Nil);
            result
        }
        Action::Aa0590d4d => {
            let result = p.b_def_method(v(0), v(1), v(3), v(4), v(5))?;
            p.local_pop();
            p.context_mut().in_def = v(2).as_context().in_def;
            p.pop_current_arg();
            result
        }
        Action::A93c61da1 => {
            let result = p.b_def_singleton(v(0), v(1), v(4), v(6), v(7), v(8))?;
            p.local_pop();
            p.context_mut().in_def = v(5).as_context().in_def;
            p.pop_current_arg();
            result
        }
        Action::Adb9388e1 => {
            p.b_keyword_cmd("break", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::A966b19eb => {
            p.b_keyword_cmd("next", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::Aa43fad97 => {
            p.b_keyword_cmd("redo", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::A5d7f07af => {
            p.b_keyword_cmd("retry", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::A84fe4440 => list(vec![v(0), Value::Context(*p.context())]),
        Action::A48aefcf2 => v(0),
        Action::A203b4421 => {
            let context = *p.context();
            if context.in_class && !context.in_def && !(context.in_block || context.in_lambda) {
                p.grammar_error("Invalid return in class/module body", &v(0))?;
            }
            result
        }
        Action::Ab39c3161 => v(1),
        Action::Afb02be77 => {
            let else_ = v(4);
            let condition =
                p.b_condition(v(0), v(1), v(2), v(3), else_.at(0), else_.at(1), Value::Nil);
            list(vec![v(0), condition])
        }
        Action::A8cf4c799 => p.b_arg(v(0))?,
        Action::A2c0631bd => push(v(0), v(2)),
        Action::A67976ae6 => concat(push(v(0), v(2)), v(4)),
        Action::A7a88e600 => {
            let mut items = vec![v(0)];
            items.extend(splat(v(2)));
            list(items)
        }
        Action::A8bcedf58 => p.b_restarg(v(0), v(1))?,
        Action::A2f0bd7c9 => p.b_restarg(v(0), Value::Nil)?,
        Action::Acc8cb959 => concat(concat(v(0), v(2)), v(3)),
        Action::Abc1f4cef => concat(concat(concat(v(0), v(2)), v(4)), v(5)),
        Action::Ae8145a70 => concat(concat(concat(concat(v(0), v(2)), v(4)), v(6)), v(7)),
        Action::A50bb5ee5 => concat(concat(v(0), v(2)), v(3)),
        Action::A26ba6e5a => {
            if v(1).as_list().is_empty() && v(0).as_list().len() == 1 {
                let arg = v(0).at(0);
                list(vec![p.b_procarg0(arg)])
            } else {
                concat(v(0), v(1))
            }
        }
        Action::Abc102fa6 => concat(v(0), v(1)),
        Action::Af5e87ec6 => p.b_args(Value::Nil, list(Vec::new()), Value::Nil)?,
        Action::Aad0ee754 => {
            p.lexer.set_state(LexState::ExprValue);
            result
        }
        Action::Af0965e7c => {
            p.max_numparam_stack.has_ordinary_params();
            p.current_arg_set(CurrentArg::Nil);
            p.b_args(v(0), v(1), v(2))?
        }
        Action::A83e8c7e3 => {
            p.max_numparam_stack.has_ordinary_params();
            p.current_arg_set(CurrentArg::Nil);
            p.b_args(v(0), concat(v(1), v(2)), v(3))?
        }
        Action::Af108a4ea => v(2),
        Action::A21a2f7f9 => {
            p.lexer.static_env.declare(&name(&v(0)));
            p.b_shadowarg(v(0))?
        }
        Action::A919593e1 => {
            p.lexer.static_env.extend_dynamic();
            p.max_numparam_stack.push(false);
            result
        }
        Action::A1558c0ae => {
            let args = p.numargs_or(v(1));
            let result = list(vec![args, v(3)]);
            p.max_numparam_stack.pop();
            p.lexer.static_env.unextend();
            p.lexer.cmdarg.pop();
            result
        }
        Action::A51d05898 => {
            p.max_numparam_stack.has_ordinary_params();
            p.b_args(v(0), concat(v(1), v(2)), v(3))?
        }
        Action::A4ecf11c6 => {
            if v(0).as_list().iter().any(Value::truthy) {
                p.max_numparam_stack.has_ordinary_params();
            }
            p.b_args(Value::Nil, v(0), Value::Nil)?
        }
        Action::A11e3700b => {
            let result = list(vec![v(0), v(2), v(3)]);
            p.context_mut().in_lambda = v(1).as_context().in_lambda;
            result
        }
        Action::Aba22ce31 => {
            let block = v(1);
            p.b_block(v(0), block.at(0), block.at(1), block.at(2), block.at(3))?
        }
        Action::A0981cb7d => {
            let args = v(3);
            p.b_call_method(v(0), v(1), v(2), args.at(0), args.at(1), args.at(2))
        }
        Action::A22b49fd6 => {
            let args = v(3);
            let call = p.b_call_method(v(0), v(1), v(2), args.at(0), args.at(1), args.at(2));
            let block = v(4);
            p.b_block(call, block.at(0), block.at(1), block.at(2), block.at(3))?
        }
        Action::A527c0661 => {
            let args = v(1);
            p.b_call_method(
                Value::Nil,
                Value::Nil,
                v(0),
                args.at(0),
                args.at(1),
                args.at(2),
            )
        }
        Action::Aab3f74d7 => {
            p.b_call_method(v(0), v(1), v(2), Value::Nil, list(Vec::new()), Value::Nil)
        }
        Action::Aca95bf6a => {
            let args = v(2);
            p.b_call_method(v(0), v(1), Value::Nil, args.at(0), args.at(1), args.at(2))
        }
        Action::A8796c317 => {
            let args = v(1);
            p.b_keyword_cmd("super", v(0), args.at(0), args.at(1), args.at(2))?
        }
        Action::A1d1cf16b => {
            p.b_keyword_cmd("zsuper", v(0), Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::A7316e5e0 => p.b_index(v(0), v(3), v(2), false),
        Action::A8dc56848 => {
            let args = p.numargs_or(v(1));
            let result = list(vec![args, v(2)]);
            p.max_numparam_stack.pop();
            p.lexer.static_env.unextend();
            result
        }
        Action::Aaebcec57 => {
            let args = p.numargs_or(v(2));
            let result = list(vec![args, v(3)]);
            p.max_numparam_stack.pop();
            p.lexer.static_env.unextend();
            p.lexer.cmdarg.pop();
            result
        }
        Action::Ad8f6c640 => {
            let when = p.b_when(v(0), v(1), v(2), v(3));
            let mut items = vec![when];
            items.extend(splat(v(4)));
            list(items)
        }
        Action::Af1765b94 => {
            p.lexer.set_state(LexState::ExprBeg);
            p.lexer.command_start = false;
            p.pattern_variables.push();
            p.pattern_hash_keys.push();
            let result = Value::Bool(p.context().in_kwarg);
            p.context_mut().in_kwarg = true;
            result
        }
        Action::A68142ac8 => {
            p.pattern_hash_keys.pop();
            p.pattern_variables.pop();
            p.set_in_kwarg(&v(1));
            result
        }
        Action::A1a2a275c => {
            let pattern = v(2);
            let in_pattern = p.b_in_pattern(v(0), pattern.at(0), pattern.at(1), v(3), v(5));
            let mut items = vec![in_pattern];
            items.extend(splat(v(6)));
            list(items)
        }
        Action::A93e7323c => list(vec![v(0), Value::Nil]),
        Action::A3736f0d1 => {
            let guard = p.b_guard("if_guard", v(1), v(2));
            list(vec![v(0), guard])
        }
        Action::Afe2388f0 => {
            let guard = p.b_guard("unless_guard", v(1), v(2));
            list(vec![v(0), guard])
        }
        Action::A48cf2658 => {
            let item = p.b_match_with_trailing_comma(v(0), v(1));
            p.b_array_pattern(Value::Nil, list(vec![item]), Value::Nil)
        }
        Action::A7b136a4c => {
            p.b_array_pattern(Value::Nil, concat(list(vec![v(0)]), v(2)), Value::Nil)
        }
        Action::A6c17ba60 => p.b_array_pattern(Value::Nil, v(0), Value::Nil),
        Action::Ab73c151c => p.b_hash_pattern(Value::Nil, v(0), Value::Nil)?,
        Action::Afd8f587a => p.b_match_binary("match_as", v(0), v(2)),
        Action::A3387c285 => p.b_match_binary("match_alt", v(0), v(2)),
        Action::Ad0565574 => {
            let result = v(0);
            p.pattern_hash_keys.push();
            result
        }
        Action::A6a06943b => {
            p.pattern_hash_keys.pop();
            let pattern = p.b_array_pattern(Value::Nil, v(2), Value::Nil);
            p.b_const_pattern(v(0), pattern, v(3))
        }
        Action::A929a25df => {
            p.pattern_hash_keys.pop();
            let pattern = p.b_hash_pattern(Value::Nil, v(2), Value::Nil)?;
            p.b_const_pattern(v(0), pattern, v(3))
        }
        Action::A6b0b77af => {
            let pattern = p.b_array_pattern(v(1), Value::Nil, v(2));
            p.b_const_pattern(v(0), pattern, v(2))
        }
        Action::A5da315f8 => {
            p.pattern_hash_keys.push();
            result
        }
        Action::A08c12e78 => {
            p.pattern_hash_keys.pop();
            p.b_array_pattern(v(0), v(2), v(3))
        }
        Action::Ac322857e => p.b_array_pattern(v(0), list(Vec::new()), v(1)),
        Action::Ae1d806cc => {
            p.pattern_hash_keys.push();
            let result = Value::Bool(p.context().in_kwarg);
            p.context_mut().in_kwarg = false;
            result
        }
        Action::Ac06d0a92 => {
            p.pattern_hash_keys.pop();
            p.set_in_kwarg(&v(1));
            p.b_hash_pattern(v(0), v(2), v(3))?
        }
        Action::A2542d91f => p.b_hash_pattern(v(0), list(Vec::new()), v(1))?,
        Action::A5532b1ff => {
            p.pattern_hash_keys.pop();
            p.b_begin(v(0), v(2), v(3))
        }
        Action::A7f1063c7 => push(v(0), v(1)),
        Action::A19e23ea1 => {
            let rest = p.b_match_rest(v(1), v(2))?;
            push(v(0), rest)
        }
        Action::Aa870de87 => {
            let rest = p.b_match_rest(v(1), v(2))?;
            concat(push(v(0), rest), v(4))
        }
        Action::A6f8c7025 => {
            let rest = p.b_match_rest(v(1), Value::Nil)?;
            push(v(0), rest)
        }
        Action::Ab53adfad => {
            let rest = p.b_match_rest(v(1), Value::Nil)?;
            concat(push(v(0), rest), v(3))
        }
        Action::Afe2c559a => list(vec![p.b_match_with_trailing_comma(v(0), v(1))]),
        Action::A08ebba77 => {
            let last = p.b_match_with_trailing_comma(v(1), v(2));
            push(v(0), last)
        }
        Action::Ade2a1b90 => list(vec![p.b_match_rest(v(0), v(1))?]),
        Action::A8f09b359 => {
            let rest = p.b_match_rest(v(0), v(1))?;
            concat(list(vec![rest]), v(3))
        }
        Action::A1ce7b2cc => list(vec![p.b_match_rest(v(0), Value::Nil)?]),
        Action::A15272a42 => {
            let rest = p.b_match_rest(v(0), Value::Nil)?;
            concat(list(vec![rest]), v(2))
        }
        Action::A8dfcf5c0 => push(v(0), v(2)),
        Action::A804b401c => concat(v(0), v(2)),
        Action::Acb037965 => {
            let label = v(0);
            p.b_match_pair(label.at(0), label.at(1), v(1))?
        }
        Action::A51f8df8e => {
            let label = v(0);
            p.b_match_label(label.at(0), label.at(1))?
        }
        Action::Aab259dbd => list(vec![Value::Sym("label"), v(0)]),
        Action::A2e601b72 => list(vec![Value::Sym("quoted"), list(vec![v(0), v(1), v(2)])]),
        Action::A0c82eb92 => list(vec![p.b_match_rest(v(0), v(1))?]),
        Action::A764d8649 => list(vec![p.b_match_rest(v(0), Value::Nil)?]),
        Action::A56118dc0 => list(vec![p.b_match_nil_pattern(v(0), v(1))]),
        Action::A8dd4ceb8 => p.b_accessible(v(0))?,
        Action::A4f51b6d8 => {
            let var = p.b_match_var(v(0))?;
            p.b_assignable(var)?
        }
        Action::A138e5de5 => {
            let var_name = name(&v(1));
            let ident = p.b_variable("ident", v(1));
            let lvar = p.b_accessible(ident)?;
            if !p.lexer.static_env.is_declared(&var_name) {
                p.grammar_error(&format!("no such local variable: `{var_name}'"), &v(1))?;
            }
            p.b_pin(v(0), lvar)
        }
        Action::Ae441ba10 => {
            let exc_var = v(2);
            let exc_list = if v(1).truthy() {
                p.b_array(Value::Nil, v(1), Value::Nil)
            } else {
                Value::Nil
            };
            let body = p.b_rescue_body(v(0), exc_list, exc_var.at(1), v(3), v(4));
            let mut items = vec![body];
            items.extend(splat(v(5)));
            list(items)
        }
        Action::A4133a42a => list(vec![v(0), v(1)]),
        Action::A6a605d30 => p.b_string_compose(Value::Nil, v(0), Value::Nil),
        Action::Af268de85 => {
            let string = p.b_string_compose(v(0), v(1), v(2));
            p.lexer.take_dedent_level();
            string
        }
        Action::Aca1323ae => {
            let string = p.b_string(v(0))?;
            p.lexer.take_dedent_level();
            string
        }
        Action::Adceaf1cf => p.b_character(v(0))?,
        Action::Ad1d73703 => {
            let string = p.b_xstring_compose(v(0), v(1), v(2));
            p.lexer.take_dedent_level();
            string
        }
        Action::Ab0c84dc6 => {
            let options = p.b_regexp_options(v(3));
            p.b_regexp_compose(v(0), v(1), v(2), options)?
        }
        Action::Abb2224f9 => p.b_words_compose(v(0), v(1), v(2)),
        Action::Aa2cb7f46 => {
            let word = p.b_word(v(1));
            push(v(0), word)
        }
        Action::A2827caca => p.b_symbols_compose(v(0), v(1), v(2)),
        Action::A6ce756fd => {
            let item = p.b_string_internal(v(1))?;
            push(v(0), item)
        }
        Action::Ad63fa348 => {
            let item = p.b_symbol_internal(v(1))?;
            push(v(0), item)
        }
        Action::A13977d88 => p.b_string_internal(v(0))?,
        Action::Abec2ebf1 => {
            p.lexer.cmdarg.push(false);
            p.lexer.cond.push(false);
            result
        }
        Action::Aeeba0c0e => {
            p.lexer.cmdarg.pop();
            p.lexer.cond.pop();
            p.b_begin(v(0), v(2), v(3))
        }
        Action::Aac32000a => p.b_gvar(v(0))?,
        Action::Acff17c35 => p.b_variable("ivar", v(0)),
        Action::A9435e2b9 => p.b_variable("cvar", v(0)),
        Action::Af533deb8 => {
            p.lexer.set_state(LexState::ExprEnd);
            p.b_symbol(v(0))?
        }
        Action::Ab818c837 => {
            p.lexer.set_state(LexState::ExprEnd);
            p.b_symbol_compose(v(0), v(1), v(2))
        }
        Action::Ac47cc224 => p.b_unary_num(v(0), v(1)),
        Action::Ad118db01 => {
            p.lexer.set_state(LexState::ExprEnd);
            p.b_numeric("int", v(0))
        }
        Action::Ab4f3ce25 => {
            p.lexer.set_state(LexState::ExprEnd);
            p.b_numeric("float", v(0))
        }
        Action::Af6b173c0 => {
            p.lexer.set_state(LexState::ExprEnd);
            p.b_numeric("rational", v(0))
        }
        Action::A36f003dd => {
            p.lexer.set_state(LexState::ExprEnd);
            p.b_numeric("complex", v(0))
        }
        Action::A5cc384f8 => p.b_variable("ident", v(0)),
        Action::A828a5dcf => p.b_keyword_literal("nil", v(0)),
        Action::Af4175410 => p.b_keyword_literal("self", v(0)),
        Action::A0a165b16 => p.b_keyword_literal("true", v(0)),
        Action::A66b4bdeb => p.b_keyword_literal("false", v(0)),
        Action::A57dea737 => p.b_keyword_literal("__FILE__", v(0)),
        Action::A3fef0dcd => p.b_keyword_literal("__LINE__", v(0)),
        Action::A66259dda => p.b_keyword_literal("__ENCODING__", v(0)),
        Action::Afe654a53 => p.b_nth_ref(v(0)),
        Action::A01d65ee6 => p.b_back_ref(v(0)),
        Action::A485752ad => list(vec![v(0), v(2)]),
        Action::A157c9d67 => Value::Nil,
        Action::Aba094cd0 => {
            let result = p.b_args(v(0), v(1), v(2))?;
            p.lexer.set_state(LexState::ExprValue);
            result
        }
        Action::A661e9e9c => {
            let mut args = splat(v(1));
            args.push(p.b_forward_arg(v(3)));
            let result = p.b_args(v(0), list(args), v(4))?;
            p.lexer.static_env.declare_forward_args();
            result
        }
        Action::A09a78472 => {
            let result = p.b_forward_only_args(v(0), v(1), v(2));
            p.lexer.static_env.declare_forward_args();
            p.lexer.set_state(LexState::ExprValue);
            result
        }
        Action::A00a2e939 => {
            let result = Value::Bool(p.context().in_kwarg);
            p.context_mut().in_kwarg = true;
            result
        }
        Action::A6b21adff => {
            p.set_in_kwarg(&v(0));
            p.b_args(Value::Nil, v(1), Value::Nil)?
        }
        Action::A0edb9197 => {
            p.grammar_error("formal argument cannot be a constant", &v(0))?;
            result
        }
        Action::A42b4c309 => {
            p.grammar_error("formal argument cannot be an instance variable", &v(0))?;
            result
        }
        Action::A95689d9e => {
            p.grammar_error("formal argument cannot be a global variable", &v(0))?;
            result
        }
        Action::A9e76b419 => {
            p.grammar_error("formal argument cannot be a class variable", &v(0))?;
            result
        }
        Action::A45782483 => {
            p.lexer.static_env.declare(&name(&v(0)));
            p.max_numparam_stack.has_ordinary_params();
            v(0)
        }
        Action::A9338e9b7 => {
            p.current_arg_set(CurrentArg::Name(Rc::from(name(&v(0)).as_str())));
            v(0)
        }
        Action::Af58f6bf7 => {
            p.current_arg_set(CurrentArg::Zero);
            p.b_arg(v(0))?
        }
        Action::Aae6cc7c6 => {
            p.check_kwarg_name(&v(0))?;
            p.lexer.static_env.declare(&name(&v(0)));
            p.max_numparam_stack.has_ordinary_params();
            p.current_arg_set(CurrentArg::Name(Rc::from(name(&v(0)).as_str())));
            v(0)
        }
        Action::A69c4e18c => {
            p.current_arg_set(CurrentArg::Nil);
            p.b_kwoptarg(v(0), v(1))?
        }
        Action::A3edbb047 => {
            p.current_arg_set(CurrentArg::Nil);
            p.b_kwarg(v(0))?
        }
        Action::Affab5fdf => p.b_kwoptarg(v(0), v(1))?,
        Action::Af0b75464 => p.b_kwarg(v(0))?,
        Action::A23f7ac27 => list(vec![p.b_kwnilarg(v(0), v(1))]),
        Action::A98ddbbba => {
            p.lexer.static_env.declare(&name(&v(1)));
            list(vec![p.b_kwrestarg(v(0), v(1))?])
        }
        Action::Aea059bc4 => list(vec![p.b_kwrestarg(v(0), Value::Nil)?]),
        Action::A3962bcd1 => {
            p.current_arg_set(CurrentArg::Zero);
            p.b_optarg(v(0), v(2))?
        }
        Action::A2faf9fc6 => {
            p.lexer.static_env.declare(&name(&v(1)));
            list(vec![p.b_restarg(v(0), v(1))?])
        }
        Action::A0228739a => list(vec![p.b_restarg(v(0), Value::Nil)?]),
        Action::Adf085caa => {
            p.lexer.static_env.declare(&name(&v(1)));
            p.b_blockarg(v(0), v(1))?
        }
        Action::A94577fdc => p.b_pair(v(0), v(2)),
        Action::A7acb1498 => p.b_pair_keyword(v(0), v(1)),
        Action::A22720faf => p.b_pair_quoted(v(0), v(1), v(2), v(3)),
        Action::Ace674ac2 => p.b_kwsplat(v(0), v(1)),
        Action::A27e22b15 => call_op(&v(0), "dot"),
        Action::A99e1041d => call_op(&v(0), "anddot"),
        Action::A0a57c26f => {
            p.yyerrok();
            result
        }
        Action::Ad173a028 => {
            p.current_arg_stack.push(CurrentArg::Nil);
            p.max_numparam_stack.push(true);
            result
        }
        Action::A40375c56 => {
            p.pattern_variables.pop();
            p.set_in_kwarg(&v(2));
            p.b_match_binary("match_pattern", v(0), v(3))
        }
        Action::A31a6246c => {
            p.pattern_variables.pop();
            p.set_in_kwarg(&v(2));
            p.b_match_binary("match_pattern_p", v(0), v(3))
        }
        Action::A66e3fece => {
            p.local_push();
            p.current_arg_stack.push(CurrentArg::Nil);
            let result = list(vec![v(0), Value::Context(*p.context())]);
            p.context_mut().in_def = true;
            result
        }
        Action::Ac1c7810d => list(vec![v(0), v(1), v(2), v(4)]),
        Action::Abf19aa1e => {
            let head = v(0);
            let (name_t, ctx) = (head.at(1).at(0), head.at(1).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let result = p.b_def_endless_method(head.at(0), name_t, v(1), v(3))?;
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.pop_current_arg();
            result
        }
        Action::A9e773905 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(1).at(0), head.at(1).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let rescue_body = p.b_rescue_body(v(4), Value::Nil, Value::Nil, Value::Nil, v(5));
            let body = p.b_begin_body(
                v(3),
                vec![rescue_body],
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Nil,
            );
            let result = p.b_def_endless_method(head.at(0), name_t, v(1), body)?;
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.pop_current_arg();
            result
        }
        Action::A387abb07 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(3).at(0), head.at(3).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let result = p.b_def_endless_singleton(head.at(0), head.at(1), name_t, v(1), v(3))?;
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.pop_current_arg();
            result
        }
        Action::Af6a4a496 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(3).at(0), head.at(3).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let rescue_body = p.b_rescue_body(v(4), Value::Nil, Value::Nil, Value::Nil, v(5));
            let body = p.b_begin_body(
                v(3),
                vec![rescue_body],
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Nil,
            );
            let result = p.b_def_endless_singleton(head.at(0), head.at(1), name_t, v(1), body)?;
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.pop_current_arg();
            result
        }
        Action::Af6dcd9d2 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(1).at(0), head.at(1).at(1).as_context());
            let result = p.b_def_method(head.at(0), name_t, v(1), v(2), v(3))?;
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.pop_current_arg();
            result
        }
        Action::Af463df71 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(3).at(0), head.at(3).at(1).as_context());
            let result = p.b_def_singleton(head.at(0), head.at(1), name_t, v(1), v(2), v(3))?;
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.pop_current_arg();
            result
        }
        Action::Adbc81925 => {
            p.lexer.static_env.extend_dynamic();
            p.max_numparam_stack.push(false);
            let result = Value::Context(*p.context());
            p.context_mut().in_lambda = true;
            result
        }
        Action::Ac0be2825 => {
            let lambda_call = p.b_call_lambda(v(0));
            let args = p.numargs_or(v(2));
            let body = v(4);
            p.max_numparam_stack.pop();
            p.lexer.static_env.unextend();
            p.lexer.cmdarg.pop();
            p.context_mut().in_lambda = v(1).as_context().in_lambda;
            p.b_block(lambda_call, body.at(0), args, body.at(1), body.at(2))?
        }
        Action::Ade93565b => {
            p.pattern_variables.pop();
            p.pattern_hash_keys.pop();
            p.set_in_kwarg(&v(1));
            result
        }
        Action::Ada37bf1e => p.b_find_pattern(Value::Nil, v(0), Value::Nil),
        Action::A33e0d886 => {
            p.pattern_hash_keys.pop();
            let pattern = p.b_find_pattern(Value::Nil, v(2), Value::Nil);
            p.b_const_pattern(v(0), pattern, v(3))
        }
        Action::Af8de7941 => p.b_array_pattern(v(0), v(1), v(2)),
        Action::Abf85f6ed => p.b_find_pattern(v(0), v(1), v(2)),
        Action::Ad47d3507 => {
            let mut items = vec![v(0)];
            items.extend(splat(v(2)));
            items.push(v(4));
            list(items)
        }
        Action::A999204e9 => p.b_match_rest(v(0), v(1))?,
        Action::Adf52b570 => p.b_match_rest(v(0), Value::Nil)?,
        Action::Ab7215c2b => {
            let head = v(0);
            let (name_t, ctx) = (head.at(1).at(0), head.at(1).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let result = p.b_def_endless_method(head.at(0), name_t, v(1), v(3))?;
            p.local_pop();
            p.pop_current_arg();
            p.context_mut().in_def = ctx.in_def;
            result
        }
        Action::A0534c239 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(1).at(0), head.at(1).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let rescue_body = p.b_rescue_body(v(4), Value::Nil, Value::Nil, Value::Nil, v(5));
            let body = p.b_begin_body(
                v(3),
                vec![rescue_body],
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Nil,
            );
            let result = p.b_def_endless_method(head.at(0), name_t, v(1), body)?;
            p.local_pop();
            p.pop_current_arg();
            p.context_mut().in_def = ctx.in_def;
            result
        }
        Action::A51e7c90c => {
            let head = v(0);
            let (name_t, ctx) = (head.at(3).at(0), head.at(3).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let result = p.b_def_endless_singleton(head.at(0), head.at(1), name_t, v(1), v(3))?;
            p.local_pop();
            p.pop_current_arg();
            p.context_mut().in_def = ctx.in_def;
            result
        }
        Action::Ab5e85d3c => {
            let head = v(0);
            let (name_t, ctx) = (head.at(3).at(0), head.at(3).at(1).as_context());
            p.endless_method_name(&name_t)?;
            let rescue_body = p.b_rescue_body(v(4), Value::Nil, Value::Nil, Value::Nil, v(5));
            let body = p.b_begin_body(
                v(3),
                vec![rescue_body],
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Nil,
            );
            let result = p.b_def_endless_singleton(head.at(0), head.at(1), name_t, v(1), body)?;
            p.local_pop();
            p.pop_current_arg();
            p.context_mut().in_def = ctx.in_def;
            result
        }
        Action::Ab53deae7 => {
            p.pattern_variables.pop();
            p.pattern_hash_keys.pop();
            p.set_in_kwarg(&v(2));
            p.b_match_binary("match_pattern", v(0), v(3))
        }
        Action::Ad9fb89fb => {
            p.pattern_variables.pop();
            p.pattern_hash_keys.pop();
            p.set_in_kwarg(&v(2));
            p.b_match_binary("match_pattern_p", v(0), v(3))
        }
        Action::Ae254a9b9 => {
            p.lexer.set_state(LexState::ExprFname);
            p.context_mut().in_argdef = true;
            result
        }
        Action::A3a13bf77 => {
            p.context_mut().in_block = v(1).as_context().in_block;
            let mut items = vec![v(0)];
            items.extend(splat(v(2)));
            items.push(v(3));
            list(items)
        }
        Action::A37eb4c5a => {
            p.context_mut().in_defined = true;
            result
        }
        Action::Aadbeb0cd => {
            p.context_mut().in_defined = false;
            p.b_keyword_cmd("defined?", v(0), Value::Nil, list(vec![v(3)]), Value::Nil)?
        }
        Action::A32f7fec2 => {
            if !p.lexer.static_env.declared_anonymous_blockarg() {
                p.grammar_error("no anonymous block parameter", &v(0))?;
            }
            p.b_block_pass(v(0), Value::Nil)
        }
        Action::Ae4f24bd2 => {
            p.context_mut().in_defined = false;
            p.b_keyword_cmd("defined?", v(0), v(2), list(vec![v(4)]), v(5))?
        }
        Action::A7e7960a4 => {
            let head = v(0);
            let (name_t, ctx) = (head.at(1).at(0), head.at(1).at(1).as_context());
            let result = p.b_def_method(head.at(0), name_t, v(1), v(2), v(3))?;
            p.local_pop();
            p.pop_current_arg();
            p.context_mut().in_def = ctx.in_def;
            result
        }
        Action::Af73cab7a => {
            let head = v(0);
            let (name_t, ctx) = (head.at(3).at(0), head.at(3).at(1).as_context());
            let result = p.b_def_singleton(head.at(0), head.at(1), name_t, v(1), v(2), v(3))?;
            p.local_pop();
            p.pop_current_arg();
            p.context_mut().in_def = ctx.in_def;
            result
        }
        Action::A29fa6e15 => {
            let result = v(0);
            p.context_mut().in_argdef = true;
            result
        }
        Action::Af5e5481e => {
            p.context_mut().in_argdef = false;
            result
        }
        Action::Ac8298278 => {
            p.max_numparam_stack.has_ordinary_params();
            p.current_arg_set(CurrentArg::Nil);
            p.context_mut().in_argdef = false;
            p.b_args(v(0), v(1), v(2))?
        }
        Action::Ae46717b8 => {
            p.max_numparam_stack.has_ordinary_params();
            p.current_arg_set(CurrentArg::Nil);
            p.context_mut().in_argdef = false;
            p.b_args(v(0), concat(v(1), v(2)), v(3))?
        }
        Action::A4fb1fd6d => {
            p.context_mut().in_argdef = false;
            p.max_numparam_stack.has_ordinary_params();
            p.b_args(v(0), concat(v(1), v(2)), v(3))?
        }
        Action::Af798d7ca => {
            p.context_mut().in_argdef = false;
            if v(0).as_list().iter().any(Value::truthy) {
                p.max_numparam_stack.has_ordinary_params();
            }
            p.b_args(Value::Nil, v(0), Value::Nil)?
        }
        Action::A07b7b35d => {
            p.context_mut().in_lambda = v(1).as_context().in_lambda;
            list(vec![v(0), v(2), v(3)])
        }
        Action::A984fe56d => {
            let non_lvar = p.b_accessible(v(1))?;
            p.b_pin(v(0), non_lvar)
        }
        Action::Ab4497d2f => {
            let expr = p.b_begin(v(1), v(2), v(3));
            p.b_pin(v(0), expr)
        }
        Action::A58621c89 => {
            p.context_mut().in_argdef = false;
            p.b_args(Value::Nil, list(Vec::new()), Value::Nil)?
        }
        Action::A7c8496ab => {
            let result = p.b_args(v(0), v(1), v(2))?;
            p.lexer.set_state(LexState::ExprValue);
            p.context_mut().in_argdef = false;
            result
        }
        Action::Abd0b6006 => {
            let result = Value::Context(*p.context());
            p.context_mut().in_kwarg = true;
            p.context_mut().in_argdef = true;
            result
        }
        Action::A82bbe48e => {
            p.context_mut().in_kwarg = v(0).as_context().in_kwarg;
            p.context_mut().in_argdef = false;
            p.b_args(Value::Nil, v(1), Value::Nil)?
        }
        Action::A50d6c102 => {
            p.lexer.static_env.declare_forward_args();
            list(vec![p.b_forward_arg(v(0))])
        }
        Action::A24624c28 => {
            p.check_kwarg_name(&v(0))?;
            p.lexer.static_env.declare(&name(&v(0)));
            p.max_numparam_stack.has_ordinary_params();
            p.current_arg_set(CurrentArg::Name(Rc::from(name(&v(0)).as_str())));
            p.context_mut().in_argdef = false;
            v(0)
        }
        Action::Aaa863d99 => {
            p.current_arg_set(CurrentArg::Nil);
            p.context_mut().in_argdef = true;
            p.b_kwoptarg(v(0), v(1))?
        }
        Action::A8ffb7ec4 => {
            p.current_arg_set(CurrentArg::Nil);
            p.context_mut().in_argdef = true;
            p.b_kwarg(v(0))?
        }
        Action::A051342d1 => {
            p.context_mut().in_argdef = true;
            p.b_kwoptarg(v(0), v(1))?
        }
        Action::Ae0e51d1a => {
            p.context_mut().in_argdef = true;
            p.b_kwarg(v(0))?
        }
        Action::A92fe5e25 => {
            p.current_arg_set(CurrentArg::Zero);
            p.context_mut().in_argdef = true;
            p.b_optarg(v(0), v(2))?
        }
        Action::A3bcec143 => {
            p.lexer.static_env.declare_anonymous_blockarg();
            p.b_blockarg(v(0), Value::Nil)?
        }
        Action::A3c92c687 => p.b_pair_label(v(0))?,
        Action::Acf0f54f0 => {
            if !p.lexer.static_env.declared_anonymous_restarg() {
                p.grammar_error("no anonymous rest parameter", &v(0))?;
            }
            list(vec![p.b_forwarded("forwarded_restarg", v(0))])
        }
        Action::A05648330 => {
            if !p.lexer.static_env.declared_anonymous_restarg() {
                p.grammar_error("no anonymous rest parameter", &v(2))?;
            }
            let item = p.b_forwarded("forwarded_restarg", v(2));
            push(v(0), item)
        }
        Action::Ab1e5b34e => concat(push(v(0), v(1)), v(3)),
        Action::Ae8e897b4 => {
            let kwnorest = v(0);
            list(vec![p.b_match_nil_pattern(kwnorest.at(0), kwnorest.at(1))])
        }
        Action::A648fe6bd => {
            let kwnorest = v(0);
            list(vec![p.b_kwnilarg(kwnorest.at(0), kwnorest.at(1))])
        }
        Action::A480387b1 => {
            p.lexer.static_env.declare_anonymous_kwrestarg();
            list(vec![p.b_kwrestarg(v(0), Value::Nil)?])
        }
        Action::Aeb600ee2 => {
            p.lexer.static_env.declare_anonymous_restarg();
            list(vec![p.b_restarg(v(0), Value::Nil)?])
        }
        Action::Ad4aa53d7 => {
            if !p.lexer.static_env.declared_anonymous_kwrestarg() {
                p.grammar_error("no anonymous keyword rest parameter", &v(0))?;
            }
            p.b_forwarded("forwarded_kwrestarg", v(0))
        }
        // ここから ruby33.y / ruby34.y で加わった動作。
        Action::Aaa781827 => {
            p.pattern_variables.pop();
            p.pattern_hash_keys.pop();
            p.set_in_kwarg(&v(2));
            p.b_match_binary("match_pattern", v(0), v(5))
        }
        Action::A617d1edf => {
            p.pattern_variables.pop();
            p.pattern_hash_keys.pop();
            p.set_in_kwarg(&v(2));
            p.b_match_binary("match_pattern_p", v(0), v(5))
        }
        Action::A0af1f95a => {
            let call = p.b_call_method(v(0), v(1), v(2), Value::Nil, list(Vec::new()), Value::Nil);
            let body = v(4);
            p.b_block(call, v(3), body.at(0), body.at(1), v(5))?
        }
        Action::A74d326ff => {
            p.context_mut().in_defined = v(2).as_context().in_defined;
            p.b_keyword_cmd("defined?", v(0), Value::Nil, list(vec![v(3)]), Value::Nil)?
        }
        Action::A3717d7b6 => Value::Context(*p.context()),
        Action::Ad2270fea => {
            if !p.lexer.static_env.declared_anonymous_blockarg() {
                p.grammar_error("no anonymous block parameter", &v(0))?;
            }
            let context = *p.context();
            let env = &p.lexer.static_env;
            if context.in_dynamic_block()
                && context.in_def
                && env.declared_anonymous_blockarg_in_current_scope()
                && env.parent_has_anonymous_blockarg()
            {
                p.grammar_error("anonymous block parameter is also used within block", &v(0))?;
            }
            p.b_block_pass(v(0), Value::Nil)
        }
        Action::A6caa26d5 => concat(v(0), v(2)),
        Action::A636f6044 => {
            if !p.lexer.static_env.declared_anonymous_restarg() {
                p.grammar_error("no anonymous rest parameter", &v(0))?;
            }
            let context = *p.context();
            let env = &p.lexer.static_env;
            if context.in_dynamic_block()
                && context.in_def
                && env.declared_anonymous_restarg_in_current_scope()
                && env.parent_has_anonymous_restarg()
            {
                p.grammar_error("anonymous rest parameter is also used within block", &v(0))?;
            }
            list(vec![p.b_forwarded("forwarded_restarg", v(0))])
        }
        Action::Acd4e17b7 => {
            p.context_mut().in_defined = v(3).as_context().in_defined;
            p.b_keyword_cmd("defined?", v(0), v(2), list(vec![v(4)]), v(5))?
        }
        Action::Abe7fd930 => {
            p.pattern_variables.push();
            result
        }
        Action::Aaa931f77 => {
            let result = Value::Bool(p.context().in_kwarg);
            p.lexer.set_state(LexState::ExprBeg);
            p.lexer.command_start = false;
            p.context_mut().in_kwarg = true;
            result
        }
        Action::A16eb4208 => {
            let pattern = v(4);
            let in_pattern = p.b_in_pattern(v(0), pattern.at(0), pattern.at(1), v(5), v(7));
            let mut items = vec![in_pattern];
            items.extend(splat(v(8)));
            list(items)
        }
        Action::A2253a24d => {
            if !p.lexer.static_env.declared_anonymous_kwrestarg() {
                p.grammar_error("no anonymous keyword rest parameter", &v(0))?;
            }
            let context = *p.context();
            let env = &p.lexer.static_env;
            if context.in_dynamic_block()
                && context.in_def
                && env.declared_anonymous_kwrestarg_in_current_scope()
                && env.parent_has_anonymous_kwrestarg()
            {
                p.grammar_error(
                    "anonymous keyword rest parameter is also used within block",
                    &v(0),
                )?;
            }
            p.b_forwarded("forwarded_kwrestarg", v(0))
        }
        Action::Af1a7660a => {
            p.local_push();
            p.current_arg_stack.push(CurrentArg::Nil);
            let result = list(vec![v(0), Value::Context(*p.context())]);
            p.context_mut().in_def = true;
            p.context_mut().cant_return = false;
            result
        }
        Action::Ad09f05c7 => {
            p.context_mut().in_class = true;
            p.context_mut().cant_return = true;
            p.local_push();
            result
        }
        Action::Acef0ceee => {
            let k_class = v(0);
            let ctx = k_class.at(1).as_context();
            if p.context().in_def {
                p.grammar_error("class definition in method body", &k_class.at(0))?;
            }
            let superclass = v(2);
            let result = p.b_def_class(k_class.at(0), v(1), superclass.at(1), v(4), v(5));
            p.local_pop();
            p.context_mut().in_class = ctx.in_class;
            p.context_mut().cant_return = ctx.cant_return;
            result
        }
        Action::A0d431771 => {
            p.context_mut().in_def = false;
            p.context_mut().in_class = false;
            p.context_mut().cant_return = true;
            p.local_push();
            result
        }
        Action::Ac990afce => {
            let k_class = v(0);
            let ctx = k_class.at(1).as_context();
            let result = p.b_def_sclass(k_class.at(0), v(2), v(5), v(6));
            p.local_pop();
            p.context_mut().in_def = ctx.in_def;
            p.context_mut().in_class = ctx.in_class;
            p.context_mut().cant_return = ctx.cant_return;
            result
        }
        Action::A2c79d925 => {
            let k_mod = v(0);
            let ctx = k_mod.at(1).as_context();
            if p.context().in_def {
                p.grammar_error("module definition in method body", &k_mod.at(0))?;
            }
            let result = p.b_def_module(k_mod.at(0), v(1), v(3), v(4));
            p.local_pop();
            p.context_mut().in_class = ctx.in_class;
            p.context_mut().cant_return = ctx.cant_return;
            result
        }
        Action::Af2e1c9dc => {
            let context = *p.context();
            if context.cant_return && !(context.in_block || context.in_lambda) {
                p.grammar_error("Invalid return in class/module body", &v(0))?;
            }
            result
        }
    })
}
