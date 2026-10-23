//! parser gem の `Parser::Builders::Default` (RuboCop は `emit_forward_arg` と `emit_match_pattern` を
//! 立てた `RuboCop::AST::Builder` を使う) のうち、診断に効くところを移したもの。
//!
//! 公開する AST は作らない。後の検査が見るノードの種類・子・範囲 (`loc.expression` など) と、
//! builder 自身が出す error 診断、静的環境 (ローカル変数) への宣言だけを parser gem のとおりに保つ。
//! error 診断は parser gem と同じく `yyerror` で還元を中断する (`Err(YyError)`)。

use std::collections::HashMap;
use std::rc::Rc;

use crate::engine::LiteralEncoding;

use super::parser::{ActionResult, CurrentArg, Parser};
use super::regexp;
use super::value::{Loc, Node, Span, Token, TokenValue, Value, join_opt};

fn n(kind: &'static str, children: Vec<Value>, loc: Loc) -> Value {
    Value::node(Node {
        kind,
        children,
        loc,
    })
}

fn n0(kind: &'static str, loc: Loc) -> Value {
    n(kind, Vec::new(), loc)
}

fn expr_loc(expression: Option<Span>) -> Loc {
    Loc {
        expression,
        ..Loc::default()
    }
}

/// `token_map`。
fn token_map(token: &Value) -> Loc {
    expr_loc(token.loc())
}

/// `join_exprs(left, right)`。
fn join_exprs(left: &Value, right: &Value) -> Option<Span> {
    join_opt(left.expression(), right.expression())
}

/// Ruby の `parts.any?`: `nil` と `false` 以外の要素があるか。
fn any(parts: &[Value]) -> bool {
    parts.iter().any(Value::truthy)
}

fn collection_map(begin_t: &Value, parts: &[Value], end_t: &Value) -> Loc {
    let expression = if begin_t.is_nil() || end_t.is_nil() {
        if any(parts) {
            join_exprs(
                parts.first().unwrap_or(&Value::Nil),
                parts.last().unwrap_or(&Value::Nil),
            )
        } else if !begin_t.is_nil() {
            begin_t.loc()
        } else if !end_t.is_nil() {
            end_t.loc()
        } else {
            None
        }
    } else {
        join_opt(begin_t.loc(), end_t.loc())
    };
    Loc {
        expression,
        begin: begin_t.loc(),
        end: end_t.loc(),
        ..Loc::default()
    }
}

fn string_map(begin_t: &Value, parts: &[Value], end_t: &Value) -> Loc {
    if begin_t.token_str().starts_with("<<") && !begin_t.is_nil() {
        // Heredoc の expression は開始トークン (`<<~EOS`) だけ。
        Loc {
            expression: begin_t.loc(),
            ..Loc::default()
        }
    } else {
        collection_map(begin_t, parts, end_t)
    }
}

fn delimited_string_map(string_t: &Value) -> Loc {
    let range = string_t.loc();
    Loc {
        begin: range.clone().map(|r| r.start..r.start + 1),
        end: range.clone().map(|r| r.end.saturating_sub(1)..r.end),
        expression: range,
        ..Loc::default()
    }
}

fn prefix_string_map(symbol: &Value) -> Loc {
    let range = symbol.loc();
    Loc {
        begin: range.clone().map(|r| r.start..r.start + 1),
        expression: range,
        ..Loc::default()
    }
}

fn variable_map(name_t: &Value) -> Loc {
    Loc {
        name: name_t.loc(),
        expression: name_t.loc(),
        ..Loc::default()
    }
}

fn binary_op_map(left: &Value, right: &Value) -> Loc {
    expr_loc(join_exprs(left, right))
}

fn unary_op_map(op_t: &Value, arg: &Value) -> Loc {
    let expression = if arg.is_nil() {
        op_t.loc()
    } else {
        join_opt(op_t.loc(), arg.expression())
    };
    expr_loc(expression)
}

fn arg_prefix_map(op_t: &Value, name_t: &Value) -> Loc {
    let expression = if name_t.is_nil() {
        op_t.loc()
    } else {
        join_opt(op_t.loc(), name_t.loc())
    };
    Loc {
        name: name_t.loc(),
        expression,
        ..Loc::default()
    }
}

fn kwarg_map(name_t: &Value, value: &Value) -> Loc {
    let label = name_t.loc();
    let name = label.clone().map(|r| r.start..r.end.saturating_sub(1));
    let expression = if value.is_nil() {
        label
    } else {
        join_opt(label, value.expression())
    };
    Loc {
        name,
        expression,
        ..Loc::default()
    }
}

fn constant_map(scope: &Value, name_t: &Value) -> Loc {
    let expression = if scope.is_nil() {
        name_t.loc()
    } else {
        join_opt(scope.expression(), name_t.loc())
    };
    Loc {
        name: name_t.loc(),
        expression,
        ..Loc::default()
    }
}

fn send_map(
    receiver: &Value,
    selector_t: &Value,
    begin_t: &Value,
    args: &[Value],
    end_t: &Value,
) -> Loc {
    let begin_l = if !receiver.is_nil() {
        receiver.expression()
    } else if !selector_t.is_nil() {
        selector_t.loc()
    } else {
        None
    };
    let end_l = if !end_t.is_nil() {
        end_t.loc()
    } else if any(args) {
        args.last().and_then(Value::expression)
    } else if !selector_t.is_nil() {
        selector_t.loc()
    } else {
        None
    };
    Loc {
        name: selector_t.loc(),
        begin: begin_t.loc(),
        end: end_t.loc(),
        expression: join_opt(begin_l, end_l),
        ..Loc::default()
    }
}

fn keyword_map(keyword_t: &Value, begin_t: &Value, args: &[Value], end_t: &Value) -> Loc {
    let end_l = if !end_t.is_nil() {
        end_t.loc()
    } else if any(args) && args.last().is_some_and(|last| !last.is_nil()) {
        args.last().and_then(Value::expression)
    } else if any(args) && args.len() > 1 {
        args[args.len() - 2].expression()
    } else {
        keyword_t.loc()
    };
    Loc {
        keyword: keyword_t.loc(),
        begin: begin_t.loc(),
        end: end_t.loc(),
        expression: join_opt(keyword_t.loc(), end_l),
        ..Loc::default()
    }
}

fn condition_map(
    keyword_t: &Value,
    cond: &Value,
    begin_t: &Value,
    body: &Value,
    else_t: &Value,
    else_: &Value,
    end_t: &Value,
) -> Loc {
    let end_l = if !end_t.is_nil() {
        end_t.loc()
    } else if else_.expression().is_some() {
        else_.expression()
    } else if else_t.loc().is_some() {
        else_t.loc()
    } else if body.expression().is_some() {
        body.expression()
    } else if begin_t.loc().is_some() {
        begin_t.loc()
    } else {
        cond.expression()
    };
    Loc {
        keyword: keyword_t.loc(),
        begin: begin_t.loc(),
        end: end_t.loc(),
        expression: join_opt(keyword_t.loc(), end_l),
        ..Loc::default()
    }
}

fn definition_loc(keyword_t: &Value, name_t: &Value, end: Option<Span>) -> Loc {
    Loc {
        keyword: keyword_t.loc(),
        name: name_t.loc(),
        expression: join_opt(keyword_t.loc(), end),
        ..Loc::default()
    }
}

/// `/\A_[1-9]\z/`。
fn numparam_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2 && bytes[0] == b'_' && (b'1'..=b'9').contains(&bytes[1])
}

impl Parser {
    fn version_number(&self) -> u8 {
        self.version.number()
    }

    // ----- 診断 -----

    fn builder_error(
        &mut self,
        message: impl Into<String>,
        range: Option<Span>,
    ) -> ActionResult<()> {
        self.error(message.into(), range)
    }

    // ----- Literals -----

    pub(crate) fn b_keyword_literal(&mut self, kind: &'static str, token: Value) -> Value {
        n0(kind, token_map(&token))
    }

    pub(crate) fn b_numeric(&mut self, kind: &'static str, token: Value) -> Value {
        let text = Value::Str(Rc::from(token.token_str()));
        n(kind, vec![text], token_map(&token))
    }

    /// `unary_num`: 符号の位置から数値の終わりまで。
    pub(crate) fn b_unary_num(&mut self, unary_t: Value, numeric: Value) -> Value {
        match numeric.as_node() {
            Some(node) => {
                let mut node = node.clone();
                node.loc.expression = join_opt(unary_t.loc(), node.loc.expression.clone());
                Value::node(node)
            }
            None => numeric,
        }
    }

    /// RuboCop::AST::Builder#string_value は parser の符号化検査を上書きし、元の値を返す。
    /// 不正な UTF-8 でも Ruby の byte 値を保持し、ここで独自の診断を加えない。
    fn string_value(&mut self, token: &Value) -> ActionResult<Value> {
        if let Some(Token {
            value: TokenValue::Literal { bytes, encoding },
            ..
        }) = token.as_token()
        {
            return Ok(Value::Literal {
                bytes: Rc::clone(bytes),
                encoding: *encoding,
            });
        }
        Ok(Value::Str(Rc::from(token.token_str())))
    }

    pub(crate) fn b_string(&mut self, string_t: Value) -> ActionResult<Value> {
        let value = self.string_value(&string_t)?;
        Ok(n("str", vec![value], delimited_string_map(&string_t)))
    }

    pub(crate) fn b_string_internal(&mut self, string_t: Value) -> ActionResult<Value> {
        let value = self.string_value(&string_t)?;
        Ok(n("str", vec![value], token_map(&string_t)))
    }

    fn collapse_string_parts(parts: &[Value]) -> bool {
        parts.len() == 1 && matches!(parts[0].kind(), Some("str" | "dstr"))
    }

    pub(crate) fn b_string_compose(&mut self, begin_t: Value, parts: Value, end_t: Value) -> Value {
        let parts = parts.into_list();
        if Self::collapse_string_parts(&parts) {
            if begin_t.is_nil() && end_t.is_nil() {
                parts.into_iter().next().unwrap_or_default()
            } else {
                let children = parts[0]
                    .as_node()
                    .map(|node| node.children.clone())
                    .unwrap_or_default();
                let loc = string_map(&begin_t, &parts, &end_t);
                n("str", children, loc)
            }
        } else {
            let loc = string_map(&begin_t, &parts, &end_t);
            n("dstr", parts, loc)
        }
    }

    pub(crate) fn b_character(&mut self, char_t: Value) -> ActionResult<Value> {
        let value = self.string_value(&char_t)?;
        Ok(n("str", vec![value], prefix_string_map(&char_t)))
    }

    pub(crate) fn b_symbol(&mut self, symbol_t: Value) -> ActionResult<Value> {
        let value = self.string_value(&symbol_t)?;
        Ok(n("sym", vec![value], prefix_string_map(&symbol_t)))
    }

    pub(crate) fn b_symbol_internal(&mut self, symbol_t: Value) -> ActionResult<Value> {
        let value = self.string_value(&symbol_t)?;
        Ok(n("sym", vec![value], token_map(&symbol_t)))
    }

    pub(crate) fn b_symbol_compose(&mut self, begin_t: Value, parts: Value, end_t: Value) -> Value {
        let parts = parts.into_list();
        if Self::collapse_string_parts(&parts) {
            let first = parts[0]
                .as_node()
                .and_then(|node| node.children.first().cloned());
            let loc = collection_map(&begin_t, std::slice::from_ref(&parts[0]), &end_t);
            n("sym", first.into_iter().collect(), loc)
        } else {
            // `version == 18 && parts.empty?` の empty_symbol は対象外の版。
            let loc = collection_map(&begin_t, &parts, &end_t);
            n("dsym", parts, loc)
        }
    }

    pub(crate) fn b_xstring_compose(
        &mut self,
        begin_t: Value,
        parts: Value,
        end_t: Value,
    ) -> Value {
        let parts = parts.into_list();
        let loc = string_map(&begin_t, &parts, &end_t);
        n("xstr", parts, loc)
    }

    pub(crate) fn b_regexp_options(&mut self, regopt_t: Value) -> Value {
        let mut options: Vec<char> = regopt_t.token_str().chars().collect();
        options.sort_unstable();
        options.dedup();
        let children = options
            .into_iter()
            .map(|option| Value::Str(Rc::from(option.to_string())))
            .collect();
        n("regopt", children, token_map(&regopt_t))
    }

    pub(crate) fn b_regexp_compose(
        &mut self,
        begin_t: Value,
        parts: Value,
        end_t: Value,
        options: Value,
    ) -> ActionResult<Value> {
        let mut parts = parts.into_list();
        if let Some((source, encoding)) = Self::static_string_bytes(&parts)
            && let Err(message) = regexp::validate(&source, encoding, &Self::regexp_flags(&options))
        {
            self.builder_error(message, join_opt(begin_t.loc(), end_t.loc()))?;
        }
        let expression = join_opt(begin_t.loc(), options.expression());
        parts.push(options);
        Ok(n(
            "regexp",
            parts,
            Loc {
                begin: begin_t.loc(),
                end: end_t.loc(),
                expression,
                ..Loc::default()
            },
        ))
    }

    fn regexp_flags(options: &Value) -> String {
        options
            .as_node()
            .map(|node| {
                node.children
                    .iter()
                    .filter_map(|child| match child {
                        Value::Str(flag) => Some(flag.to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 静的な文字列部品のバイトと符号化を連結する。補間の begin も反復走査する。
    fn static_string_bytes(nodes: &[Value]) -> Option<(Vec<u8>, LiteralEncoding)> {
        let mut out = Vec::new();
        let mut encoding = LiteralEncoding::SevenBit;
        let mut first = true;
        let mut ascii = true;
        let mut pending: Vec<_> = nodes.iter().rev().collect();
        while let Some(value) = pending.pop() {
            let node = value.as_node()?;
            match node.kind {
                "str" => {
                    let (bytes, part_encoding) = match node.children.first() {
                        Some(Value::Str(text)) => (text.as_bytes(), LiteralEncoding::Text),
                        Some(Value::Literal { bytes, encoding }) => (bytes.as_ref(), *encoding),
                        _ => continue,
                    };
                    // Array#join は、全 ASCII の場合も先頭部品の符号化を保つ。
                    if first {
                        encoding = part_encoding;
                        first = false;
                    }
                    let part_ascii = bytes.is_ascii();
                    // Ruby の String#<< は ASCII だけの接頭部から最初の非 ASCII 部品の
                    // 符号化へ移る。Unicode escape の UTF-8 を Binary へ戻してはいけない。
                    if ascii && !part_ascii {
                        encoding = part_encoding;
                    } else if !ascii && !part_ascii && encoding != part_encoding {
                        // 本家で符号化互換性例外になる結合からローカル変数を作らない。
                        return None;
                    }
                    ascii &= part_ascii;
                    out.extend_from_slice(bytes);
                }
                "begin" => {
                    // static_string([]) の結果も Array#join の先頭部品になる。
                    if node.children.is_empty() && first {
                        encoding = LiteralEncoding::SevenBit;
                        first = false;
                    }
                    pending.extend(node.children.iter().rev());
                }
                _ => return None,
            }
        }
        Some((out, encoding))
    }

    /// UTF-8 文字列として使う呼び出し側には、不正なバイトを置換して渡さない。
    pub(crate) fn static_string(nodes: &[Value]) -> Option<String> {
        String::from_utf8(Self::static_string_bytes(nodes)?.0).ok()
    }

    pub(crate) fn b_array(&mut self, begin_t: Value, elements: Value, end_t: Value) -> Value {
        let elements = elements.into_list();
        let loc = collection_map(&begin_t, &elements, &end_t);
        n("array", elements, loc)
    }

    pub(crate) fn b_splat(&mut self, star_t: Value, arg: Value) -> Value {
        let loc = unary_op_map(&star_t, &arg);
        if arg.is_nil() {
            n0("splat", loc)
        } else {
            n("splat", vec![arg], loc)
        }
    }

    pub(crate) fn b_word(&mut self, parts: Value) -> Value {
        let parts = parts.into_list();
        if Self::collapse_string_parts(&parts) {
            parts.into_iter().next().unwrap_or_default()
        } else {
            let loc = collection_map(&Value::Nil, &parts, &Value::Nil);
            n("dstr", parts, loc)
        }
    }

    pub(crate) fn b_words_compose(&mut self, begin_t: Value, parts: Value, end_t: Value) -> Value {
        let parts = parts.into_list();
        let loc = collection_map(&begin_t, &parts, &end_t);
        n("array", parts, loc)
    }

    pub(crate) fn b_symbols_compose(
        &mut self,
        begin_t: Value,
        parts: Value,
        end_t: Value,
    ) -> Value {
        let parts: Vec<Value> = parts
            .into_list()
            .into_iter()
            .map(|part| match part.as_node() {
                Some(node) if node.kind == "str" => Value::node(node.updated("sym")),
                Some(node) if node.kind == "dstr" => Value::node(node.updated("dsym")),
                _ => part,
            })
            .collect();
        let loc = collection_map(&begin_t, &parts, &end_t);
        n("array", parts, loc)
    }

    // ----- Hashes -----

    pub(crate) fn b_pair(&mut self, key: Value, value: Value) -> Value {
        let loc = binary_op_map(&key, &value);
        n("pair", vec![key, value], loc)
    }

    pub(crate) fn b_pair_keyword(&mut self, key_t: Value, value: Value) -> Value {
        let key_range = key_t.loc();
        let key_loc = expr_loc(key_range.clone().map(|r| r.start..r.end.saturating_sub(1)));
        let key = n(
            "sym",
            vec![Value::Str(Rc::from(key_t.token_str()))],
            key_loc,
        );
        let expression = join_opt(key_range, value.expression());
        n("pair", vec![key, value], expr_loc(expression))
    }

    pub(crate) fn b_pair_quoted(
        &mut self,
        begin_t: Value,
        parts: Value,
        end_t: Value,
        value: Value,
    ) -> Value {
        // pair_quoted_map: 終わりのトークンは閉じ引用符の位置へ縮める。
        let end_l = end_t.loc();
        let quote_l = end_l
            .clone()
            .map(|r| r.end.saturating_sub(2)..r.end.saturating_sub(1));
        let new_end = match (end_t.as_token(), quote_l) {
            (Some(token), Some(range)) => Value::Token(Rc::new(Token {
                tok: token.tok,
                value: token.value.clone(),
                range,
            })),
            _ => end_t.clone(),
        };
        let expression = join_opt(begin_t.loc(), value.expression());
        let key = self.b_symbol_compose(begin_t, parts, new_end);
        n("pair", vec![key, value], expr_loc(expression))
    }

    /// `pair_label` (`{x:}`)。
    pub(crate) fn b_pair_label(&mut self, key_t: Value) -> ActionResult<Value> {
        let key_l = key_t.loc();
        let value_l = key_l.clone().map(|r| r.start..r.end.saturating_sub(1));
        let label = key_t.token_str().to_owned();
        let value = if label.chars().next().is_some_and(char::is_uppercase) {
            n(
                "const",
                vec![Value::Nil, Value::Str(Rc::from(label.as_str()))],
                Loc {
                    name: value_l.clone(),
                    expression: value_l,
                    ..Loc::default()
                },
            )
        } else {
            n(
                "ident",
                vec![Value::Str(Rc::from(label.as_str()))],
                Loc {
                    name: value_l.clone(),
                    expression: value_l,
                    ..Loc::default()
                },
            )
        };
        let value = self.b_accessible(value)?;
        Ok(self.b_pair_keyword(key_t, value))
    }

    pub(crate) fn b_kwsplat(&mut self, dstar_t: Value, arg: Value) -> Value {
        let loc = unary_op_map(&dstar_t, &arg);
        n("kwsplat", vec![arg], loc)
    }

    /// `associate`: 重複したキーは warning だけ。
    pub(crate) fn b_associate(&mut self, begin_t: Value, pairs: Value, end_t: Value) -> Value {
        let pairs = pairs.into_list();
        let loc = collection_map(&begin_t, &pairs, &end_t);
        n("hash", pairs, loc)
    }

    // ----- Ranges -----

    pub(crate) fn b_range(
        &mut self,
        kind: &'static str,
        lhs: Value,
        dot_t: Value,
        rhs: Value,
    ) -> Value {
        let expression = match (lhs.is_nil(), rhs.is_nil()) {
            (false, false) => join_exprs(&lhs, &rhs),
            (false, true) => join_opt(lhs.expression(), dot_t.loc()),
            (true, false) => join_opt(dot_t.loc(), rhs.expression()),
            (true, true) => None,
        };
        n(kind, vec![lhs, rhs], expr_loc(expression))
    }

    // ----- Access -----

    pub(crate) fn b_variable(&mut self, kind: &'static str, token: Value) -> Value {
        let name = Value::Str(Rc::from(token.token_str()));
        n(kind, vec![name], variable_map(&token))
    }

    pub(crate) fn b_gvar(&mut self, token: Value) -> ActionResult<Value> {
        let name = token.token_str().to_owned();
        if name.starts_with("$0") && name.chars().count() > 2 {
            self.builder_error(
                format!("`{name}' is not allowed as a global variable name"),
                token.loc(),
            )?;
        }
        Ok(self.b_variable("gvar", token))
    }

    pub(crate) fn b_back_ref(&mut self, token: Value) -> Value {
        let name = Value::Str(Rc::from(token.token_str()));
        n("back_ref", vec![name], token_map(&token))
    }

    pub(crate) fn b_nth_ref(&mut self, token: Value) -> Value {
        let name = Value::Str(Rc::from(token.token_str()));
        n("nth_ref", vec![name], token_map(&token))
    }

    /// `accessible`。
    pub(crate) fn b_accessible(&mut self, node: Value) -> ActionResult<Value> {
        let Some(current) = node.as_node().cloned() else {
            return Ok(node);
        };
        match current.kind {
            // `emit_file_line_as_literals` は真、`emit_encoding` は偽。
            "__FILE__" => Ok(Value::node(current.updated("str"))),
            "__LINE__" => Ok(Value::node(current.updated("int"))),
            "__ENCODING__" => Ok(Value::node(current.updated("const"))),
            "ident" => {
                let name = current.name().unwrap_or_default().to_owned();
                if name.ends_with('?') || name.ends_with('!') {
                    self.builder_error(
                        format!("identifier {name} is not valid to get"),
                        current.expression(),
                    )?;
                }
                if self.try_declare_numparam(&current)? {
                    return Ok(Value::node(current.updated("lvar")));
                }
                if !self.lexer.static_env.is_declared(&name) {
                    // `it` の warning は 3.3 の文法だけ。
                    let loc = expr_loc(current.expression());
                    return Ok(n(
                        "send",
                        vec![Value::Nil, Value::Str(Rc::from(name.as_str()))],
                        loc,
                    ));
                }
                if self.current_arg_stack.last() == Some(&CurrentArg::Name(Rc::from(name.as_str())))
                {
                    self.builder_error(
                        format!("circular argument reference {name}"),
                        current.expression(),
                    )?;
                }
                Ok(Value::node(current.updated("lvar")))
            }
            _ => Ok(node),
        }
    }

    /// `try_declare_numparam` (文法ファイルの inner 節)。
    fn try_declare_numparam(&mut self, node: &Node) -> ActionResult<bool> {
        let name = node.name().unwrap_or_default().to_owned();
        if !(numparam_name(&name)
            && !self.lexer.static_env.is_declared(&name)
            && self.context().in_dynamic_block())
        {
            return Ok(false);
        }
        let location = node.expression();
        if self.max_numparam_stack.is_ordinary_params() {
            self.error("ordinary parameter is defined".to_owned(), location.clone())?;
        }
        let outer: Vec<(i32, bool)> = {
            let mut stack = self.max_numparam_stack.stack.clone();
            stack.pop();
            stack
        };
        for &(value, is_static) in outer.iter().rev() {
            if is_static {
                break;
            }
            if value > 0 {
                self.error(
                    "numbered parameter is already used in an outer scope".to_owned(),
                    location.clone(),
                )?;
            }
        }
        self.lexer.static_env.declare(&name);
        self.max_numparam_stack
            .register(i32::from(name.as_bytes()[1] - b'0'));
        Ok(true)
    }

    pub(crate) fn b_const(&mut self, name_t: Value) -> Value {
        let name = Value::Str(Rc::from(name_t.token_str()));
        let loc = constant_map(&Value::Nil, &name_t);
        n("const", vec![Value::Nil, name], loc)
    }

    pub(crate) fn b_const_global(&mut self, colon3_t: Value, name_t: Value) -> Value {
        let cbase = n0("cbase", token_map(&colon3_t));
        let name = Value::Str(Rc::from(name_t.token_str()));
        let loc = constant_map(&cbase, &name_t);
        n("const", vec![cbase, name], loc)
    }

    pub(crate) fn b_const_fetch(&mut self, scope: Value, name_t: Value) -> Value {
        let name = Value::Str(Rc::from(name_t.token_str()));
        let loc = constant_map(&scope, &name_t);
        n("const", vec![scope, name], loc)
    }

    // ----- Assignment -----

    /// `assignable`。
    pub(crate) fn b_assignable(&mut self, node: Value) -> ActionResult<Value> {
        let Some(current) = node.as_node().cloned() else {
            return Ok(Value::Nil);
        };
        Ok(match current.kind {
            "cvar" => Value::node(current.updated("cvasgn")),
            "ivar" => Value::node(current.updated("ivasgn")),
            "gvar" => Value::node(current.updated("gvasgn")),
            "const" => {
                if self.context().in_def {
                    self.builder_error("dynamic constant assignment", current.expression())?;
                }
                Value::node(current.updated("casgn"))
            }
            "ident" => {
                let name = current.name().unwrap_or_default().to_owned();
                self.check_assignment_to_numparam(&name, current.expression())?;
                self.check_reserved_for_numparam(&name, current.expression())?;
                self.lexer.static_env.declare(&name);
                Value::node(current.updated("lvasgn"))
            }
            "match_var" => {
                let name = current.name().unwrap_or_default().to_owned();
                self.check_assignment_to_numparam(&name, current.expression())?;
                self.check_reserved_for_numparam(&name, current.expression())?;
                node
            }
            "nil" | "self" | "true" | "false" | "__FILE__" | "__LINE__" | "__ENCODING__" => {
                self.builder_error("cannot assign to a keyword", current.expression())?;
                Value::Nil
            }
            "back_ref" | "nth_ref" => {
                self.builder_error(
                    "cannot assign to a back-reference variable",
                    current.expression(),
                )?;
                Value::Nil
            }
            _ => Value::Nil,
        })
    }

    pub(crate) fn b_const_op_assignable(&mut self, node: Value) -> Value {
        match node.as_node() {
            Some(current) => Value::node(current.updated("casgn")),
            None => node,
        }
    }

    pub(crate) fn b_assign(&mut self, lhs: Value, rhs: Value) -> Value {
        let Some(current) = lhs.as_node() else {
            return Value::Nil;
        };
        let mut node = current.clone();
        node.loc.expression = join_exprs(&lhs, &rhs);
        node.children.push(rhs);
        Value::node(node)
    }

    pub(crate) fn b_op_assign(
        &mut self,
        lhs: Value,
        op_t: Value,
        rhs: Value,
    ) -> ActionResult<Value> {
        let kind = lhs.kind().unwrap_or("");
        match kind {
            "gvasgn" | "ivasgn" | "lvasgn" | "cvasgn" | "casgn" | "send" | "csend" | "index" => {
                let expression = join_exprs(&lhs, &rhs);
                let operator = op_t.token_str();
                let kind = match operator {
                    "&&" => "and_asgn",
                    "||" => "or_asgn",
                    _ => "op_asgn",
                };
                Ok(n(kind, vec![lhs, rhs], expr_loc(expression)))
            }
            "back_ref" | "nth_ref" => {
                self.builder_error(
                    "cannot assign to a back-reference variable",
                    lhs.expression(),
                )?;
                Ok(Value::Nil)
            }
            _ => Ok(Value::Nil),
        }
    }

    pub(crate) fn b_multi_lhs(&mut self, begin_t: Value, items: Value, end_t: Value) -> Value {
        let items = items.into_list();
        let loc = collection_map(&begin_t, &items, &end_t);
        n("mlhs", items, loc)
    }

    pub(crate) fn b_multi_assign(&mut self, lhs: Value, rhs: Value) -> Value {
        let loc = binary_op_map(&lhs, &rhs);
        n("masgn", vec![lhs, rhs], loc)
    }

    // ----- Class and module definition -----

    pub(crate) fn b_def_class(
        &mut self,
        class_t: Value,
        name: Value,
        superclass: Value,
        body: Value,
        end_t: Value,
    ) -> Value {
        let loc = definition_loc(&class_t, &Value::Nil, end_t.loc());
        n("class", vec![name, superclass, body], loc)
    }

    pub(crate) fn b_def_sclass(
        &mut self,
        class_t: Value,
        expr: Value,
        body: Value,
        end_t: Value,
    ) -> Value {
        let loc = definition_loc(&class_t, &Value::Nil, end_t.loc());
        n("sclass", vec![expr, body], loc)
    }

    pub(crate) fn b_def_module(
        &mut self,
        module_t: Value,
        name: Value,
        body: Value,
        end_t: Value,
    ) -> Value {
        let loc = definition_loc(&module_t, &Value::Nil, end_t.loc());
        n("module", vec![name, body], loc)
    }

    // ----- Method (un)definition -----

    pub(crate) fn b_def_method(
        &mut self,
        def_t: Value,
        name_t: Value,
        args: Value,
        body: Value,
        end_t: Value,
    ) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let loc = definition_loc(&def_t, &name_t, end_t.loc());
        let name = Value::Str(Rc::from(name_t.token_str()));
        Ok(n("def", vec![name, args, body], loc))
    }

    pub(crate) fn b_def_endless_method(
        &mut self,
        def_t: Value,
        name_t: Value,
        args: Value,
        body: Value,
    ) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let loc = definition_loc(&def_t, &name_t, body.expression());
        let name = Value::Str(Rc::from(name_t.token_str()));
        Ok(n("def", vec![name, args, body], loc))
    }

    pub(crate) fn b_def_singleton(
        &mut self,
        def_t: Value,
        definee: Value,
        name_t: Value,
        args: Value,
        body: Value,
        end_t: Value,
    ) -> ActionResult<Value> {
        self.validate_definee(&definee)?;
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let loc = definition_loc(&def_t, &name_t, end_t.loc());
        let name = Value::Str(Rc::from(name_t.token_str()));
        Ok(n("defs", vec![definee, name, args, body], loc))
    }

    pub(crate) fn b_def_endless_singleton(
        &mut self,
        def_t: Value,
        definee: Value,
        name_t: Value,
        args: Value,
        body: Value,
    ) -> ActionResult<Value> {
        self.validate_definee(&definee)?;
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let loc = definition_loc(&def_t, &name_t, body.expression());
        let name = Value::Str(Rc::from(name_t.token_str()));
        Ok(n("defs", vec![definee, name, args, body], loc))
    }

    pub(crate) fn b_undef_method(&mut self, undef_t: Value, names: Value) -> Value {
        let names = names.into_list();
        let loc = keyword_map(&undef_t, &Value::Nil, &names, &Value::Nil);
        n("undef", names, loc)
    }

    pub(crate) fn b_alias(&mut self, alias_t: Value, to: Value, from: Value) -> Value {
        let loc = keyword_map(
            &alias_t,
            &Value::Nil,
            &[to.clone(), from.clone()],
            &Value::Nil,
        );
        n("alias", vec![to, from], loc)
    }

    // ----- Formal arguments -----

    pub(crate) fn b_args(
        &mut self,
        begin_t: Value,
        args: Value,
        end_t: Value,
    ) -> ActionResult<Value> {
        let args = args.into_list();
        self.check_duplicate_args(&args, &mut HashMap::new())?;
        self.validate_no_forward_arg_after_restarg(&args)?;
        // `emit_forward_arg` が真なので forward_args にはしない。
        let loc = collection_map(&begin_t, &args, &end_t);
        Ok(n("args", args, loc))
    }

    pub(crate) fn b_numargs(&mut self, max_numparam: i32) -> Value {
        n(
            "numargs",
            vec![Value::Int(i64::from(max_numparam))],
            Loc::default(),
        )
    }

    pub(crate) fn b_forward_only_args(
        &mut self,
        begin_t: Value,
        dots_t: Value,
        end_t: Value,
    ) -> Value {
        let arg = self.b_forward_arg(dots_t);
        let loc = collection_map(&begin_t, std::slice::from_ref(&arg), &end_t);
        n("args", vec![arg], loc)
    }

    pub(crate) fn b_forward_arg(&mut self, dots_t: Value) -> Value {
        n0("forward_arg", token_map(&dots_t))
    }

    pub(crate) fn b_arg(&mut self, name_t: Value) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        Ok(self.b_variable("arg", name_t))
    }

    pub(crate) fn b_optarg(&mut self, name_t: Value, value: Value) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let name = Value::Str(Rc::from(name_t.token_str()));
        let loc = Loc {
            name: name_t.loc(),
            expression: join_opt(name_t.loc(), value.expression()),
            ..Loc::default()
        };
        Ok(n("optarg", vec![name, value], loc))
    }

    pub(crate) fn b_restarg(&mut self, star_t: Value, name_t: Value) -> ActionResult<Value> {
        let loc = arg_prefix_map(&star_t, &name_t);
        if name_t.is_nil() {
            Ok(n0("restarg", loc))
        } else {
            self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
            Ok(n(
                "restarg",
                vec![Value::Str(Rc::from(name_t.token_str()))],
                loc,
            ))
        }
    }

    pub(crate) fn b_kwarg(&mut self, name_t: Value) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let loc = kwarg_map(&name_t, &Value::Nil);
        Ok(n(
            "kwarg",
            vec![Value::Str(Rc::from(name_t.token_str()))],
            loc,
        ))
    }

    pub(crate) fn b_kwoptarg(&mut self, name_t: Value, value: Value) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        let loc = kwarg_map(&name_t, &value);
        Ok(n(
            "kwoptarg",
            vec![Value::Str(Rc::from(name_t.token_str())), value],
            loc,
        ))
    }

    pub(crate) fn b_kwrestarg(&mut self, dstar_t: Value, name_t: Value) -> ActionResult<Value> {
        let loc = arg_prefix_map(&dstar_t, &name_t);
        if name_t.is_nil() {
            Ok(n0("kwrestarg", loc))
        } else {
            self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
            Ok(n(
                "kwrestarg",
                vec![Value::Str(Rc::from(name_t.token_str()))],
                loc,
            ))
        }
    }

    pub(crate) fn b_kwnilarg(&mut self, dstar_t: Value, nil_t: Value) -> Value {
        n0("kwnilarg", arg_prefix_map(&dstar_t, &nil_t))
    }

    pub(crate) fn b_shadowarg(&mut self, name_t: Value) -> ActionResult<Value> {
        self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        Ok(self.b_variable("shadowarg", name_t))
    }

    pub(crate) fn b_blockarg(&mut self, amper_t: Value, name_t: Value) -> ActionResult<Value> {
        if !name_t.is_nil() {
            self.check_reserved_for_numparam(name_t.token_str(), name_t.loc())?;
        }
        let name = if name_t.is_nil() {
            Value::Nil
        } else {
            Value::Str(Rc::from(name_t.token_str()))
        };
        Ok(n("blockarg", vec![name], arg_prefix_map(&amper_t, &name_t)))
    }

    /// `procarg0`: `emit_procarg0` は偽なので引数をそのまま返す。
    pub(crate) fn b_procarg0(&mut self, arg: Value) -> Value {
        arg
    }

    // ----- Method calls -----

    pub(crate) fn b_forwarded(&mut self, kind: &'static str, token: Value) -> Value {
        n0(kind, token_map(&token))
    }

    fn call_type_for_dot(dot_t: &Value) -> &'static str {
        match dot_t.as_token() {
            Some(token) if token.value == TokenValue::Sym("anddot") => "csend",
            _ => "send",
        }
    }

    pub(crate) fn b_call_method(
        &mut self,
        receiver: Value,
        dot_t: Value,
        selector_t: Value,
        lparen_t: Value,
        args: Value,
        rparen_t: Value,
    ) -> Value {
        let kind = Self::call_type_for_dot(&dot_t);
        let args = args.into_list();
        let loc = send_map(&receiver, &selector_t, &lparen_t, &args, &rparen_t);
        let selector = if selector_t.is_nil() {
            Value::Str(Rc::from("call"))
        } else {
            Value::Str(Rc::from(selector_t.token_str()))
        };
        let mut children = vec![receiver, selector];
        children.extend(args);
        n(kind, children, loc)
    }

    /// `call_lambda`: `emit_lambda` は偽。
    pub(crate) fn b_call_lambda(&mut self, lambda_t: Value) -> Value {
        let loc = send_map(&Value::Nil, &lambda_t, &Value::Nil, &[], &Value::Nil);
        n(
            "send",
            vec![Value::Nil, Value::Str(Rc::from("lambda"))],
            loc,
        )
    }

    /// `block`。
    pub(crate) fn b_block(
        &mut self,
        method_call: Value,
        _begin_t: Value,
        args: Value,
        body: Value,
        end_t: Value,
    ) -> ActionResult<Value> {
        let call = method_call.as_node().cloned().unwrap_or(Node {
            kind: "nil",
            children: Vec::new(),
            loc: Loc::default(),
        });
        if call.kind == "yield" {
            self.builder_error("block given to yield", call.loc.keyword.clone())?;
        }
        let last_arg = if call.kind == "super" || call.children.len() > 2 {
            call.children.last().cloned()
        } else {
            None
        };
        // 3.3 の文法だけは super に限ってブロック引数とブロックの併用を許す。
        let checks_blockarg = self.version_number() != 33 || call.kind != "super";
        if let Some(last) = last_arg
            && checks_blockarg
            && matches!(last.kind(), Some("block_pass" | "forwarded_args"))
        {
            self.builder_error(
                "both block argument and literal block are passed",
                last.expression(),
            )?;
        }
        let (block_type, args) = match args.as_node() {
            Some(node) if node.kind == "numargs" => (
                "numblock",
                node.children.first().cloned().unwrap_or_default(),
            ),
            _ => ("block", args),
        };
        if matches!(
            call.kind,
            "send" | "csend" | "index" | "super" | "zsuper" | "lambda"
        ) {
            let expression = join_opt(call.expression(), end_t.loc());
            Ok(n(
                block_type,
                vec![method_call, args, body],
                expr_loc(expression),
            ))
        } else {
            // `return foo 1 do end` は (return) に block を付け替える。
            let actual_send = call.children.first().cloned().unwrap_or_default();
            let expression = join_opt(actual_send.expression(), end_t.loc());
            let block = n(
                block_type,
                vec![actual_send, args, body],
                expr_loc(expression),
            );
            let mut outer = call.clone();
            outer.loc.expression = join_opt(call.expression(), block.expression());
            outer.children = vec![block];
            Ok(Value::node(outer))
        }
    }

    pub(crate) fn b_block_pass(&mut self, amper_t: Value, arg: Value) -> Value {
        let loc = unary_op_map(&amper_t, &arg);
        n("block_pass", vec![arg], loc)
    }

    pub(crate) fn b_attr_asgn(
        &mut self,
        receiver: Value,
        dot_t: Value,
        selector_t: Value,
    ) -> Value {
        let kind = Self::call_type_for_dot(&dot_t);
        let loc = send_map(&receiver, &selector_t, &Value::Nil, &[], &Value::Nil);
        let name = Value::Str(Rc::from(format!("{}=", selector_t.token_str())));
        n(kind, vec![receiver, name], loc)
    }

    /// `index` / `index_asgn`: `emit_index` は偽なので send。
    pub(crate) fn b_index(
        &mut self,
        receiver: Value,
        rbrack_t: Value,
        indexes: Value,
        asgn: bool,
    ) -> Value {
        let expression = join_opt(receiver.expression(), rbrack_t.loc());
        let selector = if asgn { "[]=" } else { "[]" };
        let mut children = vec![receiver, Value::Str(Rc::from(selector))];
        children.extend(indexes.into_list());
        n("send", children, expr_loc(expression))
    }

    pub(crate) fn b_binary_op(&mut self, receiver: Value, operator_t: Value, arg: Value) -> Value {
        let expression = join_exprs(&receiver, &arg);
        let operator = Value::Str(Rc::from(operator_t.token_str()));
        n("send", vec![receiver, operator, arg], expr_loc(expression))
    }

    /// `match_op`: 補間の無い正規表現の名前付きキャプチャはローカル変数になる。
    pub(crate) fn b_match_op(&mut self, receiver: Value, arg: Value) -> Value {
        let expression = join_exprs(&receiver, &arg);
        if let Some(names) = self.static_regexp_names(&receiver) {
            for name in names {
                self.lexer.static_env.declare(&name);
            }
            n(
                "match_with_lvasgn",
                vec![receiver, arg],
                expr_loc(expression),
            )
        } else {
            n(
                "send",
                vec![receiver, Value::Str(Rc::from("=~")), arg],
                expr_loc(expression),
            )
        }
    }

    /// `static_regexp_node(node).names`。コンパイル診断は別の未対応項目だが、
    /// 静的な binary 正規表現の捕捉名を UTF-8 の変換失敗で落とさない。
    fn static_regexp_names(&self, node: &Value) -> Option<Vec<String>> {
        let node = node.as_node().filter(|node| node.kind == "regexp")?;
        let (options, parts) = node.children.split_last()?;
        // 3.3 以降の文法は、補間を含む正規表現 (`#{""}` のように静的に展開できるものも) を見ない。
        if self.version_number() >= 33 && parts.iter().any(|part| part.kind() != Some("str")) {
            return None;
        }
        let (bytes, encoding) = Self::static_string_bytes(parts)?;
        regexp::validate(&bytes, encoding, &Self::regexp_flags(options)).ok()?;
        // binary の source_pts は各バイトを1文字として保持するので、名前も同じ表現へ戻す。
        // メタ文字は ASCII のままで、不正 UTF-8 バイトを動的補間と取り違えない。
        let source = if encoding == LiteralEncoding::Binary {
            bytes.into_iter().map(char::from).collect()
        } else {
            String::from_utf8(bytes).ok()?
        };
        Some(regexp::names(&source, &Self::regexp_flags(options)))
    }

    pub(crate) fn b_unary_op(&mut self, op_t: Value, receiver: Value) -> Value {
        let expression = join_opt(op_t.loc(), receiver.expression());
        let method = match op_t.token_str() {
            "+" => "+@".to_owned(),
            "-" => "-@".to_owned(),
            other => other.to_owned(),
        };
        n(
            "send",
            vec![receiver, Value::Str(Rc::from(method))],
            expr_loc(expression),
        )
    }

    pub(crate) fn b_not_op(
        &mut self,
        not_t: Value,
        begin_t: Value,
        receiver: Value,
        end_t: Value,
    ) -> Value {
        if receiver.is_nil() {
            let nil_node = n0("begin", collection_map(&begin_t, &[], &end_t));
            let expression = join_opt(not_t.loc(), nil_node.expression());
            n(
                "send",
                vec![nil_node, Value::Str(Rc::from("!"))],
                expr_loc(expression),
            )
        } else {
            let loc = send_map(
                &Value::Nil,
                &not_t,
                &begin_t,
                std::slice::from_ref(&receiver),
                &end_t,
            );
            n("send", vec![receiver, Value::Str(Rc::from("!"))], loc)
        }
    }

    // ----- Control flow -----

    pub(crate) fn b_logical_op(&mut self, kind: &'static str, lhs: Value, rhs: Value) -> Value {
        let loc = binary_op_map(&lhs, &rhs);
        n(kind, vec![lhs, rhs], loc)
    }

    /// `condition`。`check_condition` は 2.4 以降で診断を出さず、後の検査も見ないので移さない。
    // parser gem の builder の位置引数と対応を保つ。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn b_condition(
        &mut self,
        cond_t: Value,
        cond: Value,
        then_t: Value,
        if_true: Value,
        else_t: Value,
        if_false: Value,
        end_t: Value,
    ) -> Value {
        let loc = condition_map(
            &cond_t, &cond, &then_t, &if_true, &else_t, &if_false, &end_t,
        );
        n("if", vec![cond, if_true, if_false], loc)
    }

    pub(crate) fn b_condition_mod(
        &mut self,
        if_true: Value,
        if_false: Value,
        cond: Value,
    ) -> Value {
        let pre = if if_true.is_nil() {
            &if_false
        } else {
            &if_true
        };
        let expression = join_exprs(pre, &cond);
        n("if", vec![cond, if_true, if_false], expr_loc(expression))
    }

    pub(crate) fn b_ternary(&mut self, cond: Value, if_true: Value, if_false: Value) -> Value {
        let expression = join_exprs(&cond, &if_false);
        n("if", vec![cond, if_true, if_false], expr_loc(expression))
    }

    pub(crate) fn b_when(
        &mut self,
        when_t: Value,
        patterns: Value,
        then_t: Value,
        body: Value,
    ) -> Value {
        let mut children = patterns.into_list();
        children.push(body);
        let loc = keyword_map(&when_t, &then_t, &children, &Value::Nil);
        n("when", children, loc)
    }

    // parser gem の builder の位置引数と対応を保つ。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn b_case(
        &mut self,
        kind: &'static str,
        case_t: Value,
        expr: Value,
        bodies: Vec<Value>,
        else_t: Value,
        else_body: Value,
        end_t: Value,
    ) -> Value {
        let else_body = if kind == "case_match" && !else_t.is_nil() && else_body.is_nil() {
            n0("empty_else", token_map(&else_t))
        } else {
            else_body
        };
        let loc = condition_map(
            &case_t,
            &expr,
            &Value::Nil,
            &Value::Nil,
            &else_t,
            &else_body,
            &end_t,
        );
        let mut children = vec![expr];
        children.extend(bodies);
        children.push(else_body);
        n(kind, children, loc)
    }

    pub(crate) fn b_loop(
        &mut self,
        kind: &'static str,
        keyword_t: Value,
        cond: Value,
        do_t: Value,
        body: Value,
        end_t: Value,
    ) -> Value {
        let loc = keyword_map(&keyword_t, &do_t, &[], &end_t);
        n(kind, vec![cond, body], loc)
    }

    pub(crate) fn b_loop_mod(&mut self, kind: &'static str, body: Value, cond: Value) -> Value {
        let kind = match (kind, body.kind()) {
            ("while", Some("kwbegin")) => "while_post",
            ("until", Some("kwbegin")) => "until_post",
            _ => kind,
        };
        let expression = join_exprs(&body, &cond);
        n(kind, vec![cond, body], expr_loc(expression))
    }

    pub(crate) fn b_for(
        &mut self,
        for_t: Value,
        iterator: Value,
        iteratee: Value,
        body: Value,
        end_t: Value,
    ) -> Value {
        let expression = join_opt(for_t.loc(), end_t.loc());
        n("for", vec![iterator, iteratee, body], expr_loc(expression))
    }

    /// `keyword_cmd`。
    pub(crate) fn b_keyword_cmd(
        &mut self,
        kind: &'static str,
        keyword_t: Value,
        lparen_t: Value,
        args: Value,
        rparen_t: Value,
    ) -> ActionResult<Value> {
        let args = args.into_list();
        if kind == "yield"
            && let Some(last) = args.last()
            && last.kind() == Some("block_pass")
        {
            self.builder_error("block given to yield", keyword_t.loc())?;
        }
        let loc = keyword_map(&keyword_t, &lparen_t, &args, &rparen_t);
        Ok(n(kind, args, loc))
    }

    pub(crate) fn b_prepost_exe(
        &mut self,
        kind: &'static str,
        exe_t: Value,
        compstmt: Value,
        rbrace_t: Value,
    ) -> Value {
        let loc = keyword_map(&exe_t, &Value::Nil, &[], &rbrace_t);
        n(kind, vec![compstmt], loc)
    }

    pub(crate) fn b_rescue_body(
        &mut self,
        rescue_t: Value,
        exc_list: Value,
        exc_var: Value,
        then_t: Value,
        compound_stmt: Value,
    ) -> Value {
        let end_l = compound_stmt
            .expression()
            .or_else(|| then_t.loc())
            .or_else(|| exc_var.expression())
            .or_else(|| exc_list.expression())
            .or_else(|| rescue_t.loc());
        let expression = join_opt(rescue_t.loc(), end_l);
        n(
            "resbody",
            vec![exc_list, exc_var, compound_stmt],
            expr_loc(expression),
        )
    }

    pub(crate) fn b_begin_body(
        &mut self,
        compound_stmt: Value,
        rescue_bodies: Vec<Value>,
        else_t: Value,
        else_: Value,
        ensure_t: Value,
        ensure_: Value,
    ) -> Value {
        let mut compound = compound_stmt;
        if any(&rescue_bodies) {
            let begin_l = compound
                .expression()
                .or_else(|| rescue_bodies.first().and_then(Value::expression));
            let end_l = if !else_t.is_nil() {
                if else_.is_nil() {
                    else_t.loc()
                } else {
                    else_.expression()
                }
            } else {
                rescue_bodies.last().and_then(Value::expression)
            };
            let mut children = vec![compound];
            children.extend(rescue_bodies);
            children.push(if else_t.is_nil() { Value::Nil } else { else_ });
            compound = n("rescue", children, expr_loc(join_opt(begin_l, end_l)));
        } else if !else_t.is_nil() {
            let mut statements = Vec::new();
            match compound.as_node() {
                Some(node) if node.kind == "begin" => statements.extend(node.children.clone()),
                Some(_) => statements.push(compound.clone()),
                None => {}
            }
            let else_begin = n(
                "begin",
                vec![else_.clone()],
                collection_map(&else_t, std::slice::from_ref(&else_), &Value::Nil),
            );
            statements.push(else_begin);
            let loc = collection_map(&Value::Nil, &statements, &Value::Nil);
            compound = n("begin", statements, loc);
        }
        if !ensure_t.is_nil() {
            let begin_l = compound.expression().or_else(|| ensure_t.loc());
            let end_l = ensure_.expression().or_else(|| ensure_t.loc());
            compound = n(
                "ensure",
                vec![compound, ensure_],
                expr_loc(join_opt(begin_l, end_l)),
            );
        }
        compound
    }

    // ----- Expression grouping -----

    pub(crate) fn b_compstmt(&mut self, statements: Value) -> Value {
        let statements = statements.into_list();
        // `none?` / `one?` は真の要素を数える。`one?` のときに返すのは先頭の要素そのもの。
        let truthy = statements.iter().filter(|value| value.truthy()).count();
        if truthy == 0 {
            Value::Nil
        } else if truthy == 1 {
            statements.into_iter().next().unwrap_or_default()
        } else {
            let loc = collection_map(&Value::Nil, &statements, &Value::Nil);
            n("begin", statements, loc)
        }
    }

    pub(crate) fn b_begin(&mut self, begin_t: Value, body: Value, end_t: Value) -> Value {
        match body.as_node() {
            None => n0("begin", collection_map(&begin_t, &[], &end_t)),
            Some(node)
                if node.kind == "mlhs"
                    || (node.kind == "begin"
                        && node.loc.begin.is_none()
                        && node.loc.end.is_none()) =>
            {
                let loc = collection_map(&begin_t, &node.children, &end_t);
                n(node.kind, node.children.clone(), loc)
            }
            Some(_) => {
                let loc = collection_map(&begin_t, std::slice::from_ref(&body), &end_t);
                n("begin", vec![body], loc)
            }
        }
    }

    pub(crate) fn b_begin_keyword(&mut self, begin_t: Value, body: Value, end_t: Value) -> Value {
        match body.as_node() {
            None => n0("kwbegin", collection_map(&begin_t, &[], &end_t)),
            Some(node)
                if node.kind == "begin" && node.loc.begin.is_none() && node.loc.end.is_none() =>
            {
                let loc = collection_map(&begin_t, &node.children, &end_t);
                n("kwbegin", node.children.clone(), loc)
            }
            Some(_) => {
                let loc = collection_map(&begin_t, std::slice::from_ref(&body), &end_t);
                n("kwbegin", vec![body], loc)
            }
        }
    }

    // ----- Pattern matching -----

    pub(crate) fn b_match_binary(&mut self, kind: &'static str, lhs: Value, rhs: Value) -> Value {
        let loc = binary_op_map(&lhs, &rhs);
        n(kind, vec![lhs, rhs], loc)
    }

    pub(crate) fn b_in_pattern(
        &mut self,
        in_t: Value,
        pattern: Value,
        guard: Value,
        then_t: Value,
        body: Value,
    ) -> Value {
        let children = vec![pattern, guard, body];
        let compact: Vec<Value> = children
            .iter()
            .filter(|value| !value.is_nil())
            .cloned()
            .collect();
        let loc = keyword_map(&in_t, &then_t, &compact, &Value::Nil);
        n("in_pattern", children, loc)
    }

    pub(crate) fn b_guard(&mut self, kind: &'static str, keyword_t: Value, body: Value) -> Value {
        let expression = join_opt(keyword_t.loc(), body.expression());
        n(kind, vec![body], expr_loc(expression))
    }

    pub(crate) fn b_match_var(&mut self, name_t: Value) -> ActionResult<Value> {
        let name = name_t.token_str().to_owned();
        let name_l = name_t.loc();
        self.check_lvar_name(&name, name_l.clone())?;
        self.check_duplicate_pattern_variable(&name, name_l)?;
        self.lexer.static_env.declare(&name);
        Ok(n(
            "match_var",
            vec![Value::Str(Rc::from(name.as_str()))],
            variable_map(&name_t),
        ))
    }

    pub(crate) fn b_match_hash_var(&mut self, name_t: Value) -> ActionResult<Value> {
        let name = name_t.token_str().to_owned();
        let expr_l = name_t.loc();
        let name_l = expr_l.clone().map(|r| r.start..r.end.saturating_sub(1));
        self.check_lvar_name(&name, name_l.clone())?;
        self.check_duplicate_pattern_variable(&name, name_l.clone())?;
        self.lexer.static_env.declare(&name);
        Ok(n(
            "match_var",
            vec![Value::Str(Rc::from(name.as_str()))],
            Loc {
                name: name_l,
                expression: expr_l,
                ..Loc::default()
            },
        ))
    }

    pub(crate) fn b_match_hash_var_from_str(
        &mut self,
        begin_t: Value,
        strings: Value,
        end_t: Value,
    ) -> ActionResult<Value> {
        let strings = strings.into_list();
        let label_loc = join_opt(begin_t.loc(), end_t.loc());
        if strings.len() > 1 {
            self.builder_error(
                "symbol literal with interpolation is not allowed",
                label_loc.clone(),
            )?;
        }
        let string = strings.into_iter().next().unwrap_or_default();
        match string.as_node() {
            Some(node) if node.kind == "str" => {
                let name = node.name().unwrap_or_default().to_owned();
                let name_l = node.expression();
                self.check_lvar_name(&name, name_l.clone())?;
                self.check_duplicate_pattern_variable(&name, name_l.clone())?;
                self.lexer.static_env.declare(&name);
                let expression = join_opt(join_opt(begin_t.loc(), node.expression()), end_t.loc());
                Ok(n(
                    "match_var",
                    vec![Value::Str(Rc::from(name.as_str()))],
                    Loc {
                        name: name_l,
                        expression,
                        ..Loc::default()
                    },
                ))
            }
            Some(node) if node.kind == "begin" => {
                let children = Value::List(node.children.clone());
                self.b_match_hash_var_from_str(begin_t, children, end_t)
            }
            _ => {
                self.builder_error(
                    "symbol literal with interpolation is not allowed",
                    label_loc,
                )?;
                Ok(Value::Nil)
            }
        }
    }

    pub(crate) fn b_match_rest(&mut self, star_t: Value, name_t: Value) -> ActionResult<Value> {
        if name_t.is_nil() {
            Ok(n0("match_rest", unary_op_map(&star_t, &Value::Nil)))
        } else {
            let name = self.b_match_var(name_t)?;
            let loc = unary_op_map(&star_t, &name);
            Ok(n("match_rest", vec![name], loc))
        }
    }

    pub(crate) fn b_hash_pattern(
        &mut self,
        lbrace_t: Value,
        kwargs: Value,
        rbrace_t: Value,
    ) -> ActionResult<Value> {
        let args = kwargs.into_list();
        self.check_duplicate_args(&args, &mut HashMap::new())?;
        let loc = collection_map(&lbrace_t, &args, &rbrace_t);
        Ok(n("hash_pattern", args, loc))
    }

    pub(crate) fn b_array_pattern(
        &mut self,
        lbrack_t: Value,
        elements: Value,
        rbrack_t: Value,
    ) -> Value {
        if elements.is_nil() {
            return n(
                "array_pattern",
                Vec::new(),
                collection_map(&lbrack_t, &[], &rbrack_t),
            );
        }
        let elements = elements.into_list();
        let mut trailing_comma = false;
        let node_elements: Vec<Value> = elements
            .iter()
            .map(|element| match element.as_node() {
                Some(node) if node.kind == "match_with_trailing_comma" => {
                    trailing_comma = true;
                    node.children.first().cloned().unwrap_or_default()
                }
                _ => {
                    trailing_comma = false;
                    element.clone()
                }
            })
            .collect();
        let kind = if trailing_comma {
            "array_pattern_with_tail"
        } else {
            "array_pattern"
        };
        let loc = collection_map(&lbrack_t, &elements, &rbrack_t);
        n(kind, node_elements, loc)
    }

    pub(crate) fn b_find_pattern(
        &mut self,
        lbrack_t: Value,
        elements: Value,
        rbrack_t: Value,
    ) -> Value {
        let elements = elements.into_list();
        let loc = collection_map(&lbrack_t, &elements, &rbrack_t);
        n("find_pattern", elements, loc)
    }

    pub(crate) fn b_match_with_trailing_comma(&mut self, matched: Value, comma_t: Value) -> Value {
        let expression = join_opt(matched.expression(), comma_t.loc());
        n(
            "match_with_trailing_comma",
            vec![matched],
            expr_loc(expression),
        )
    }

    pub(crate) fn b_const_pattern(
        &mut self,
        constant: Value,
        pattern: Value,
        rdelim_t: Value,
    ) -> Value {
        let expression = join_opt(constant.expression(), rdelim_t.loc());
        n(
            "const_pattern",
            vec![constant, pattern],
            expr_loc(expression),
        )
    }

    pub(crate) fn b_pin(&mut self, pin_t: Value, var: Value) -> Value {
        let expression = join_opt(pin_t.loc(), var.expression());
        n("pin", vec![var], expr_loc(expression))
    }

    pub(crate) fn b_match_nil_pattern(&mut self, dstar_t: Value, nil_t: Value) -> Value {
        n0("match_nil_pattern", arg_prefix_map(&dstar_t, &nil_t))
    }

    /// `match_pair(label_type, label, value)`。
    pub(crate) fn b_match_pair(
        &mut self,
        label_type: Value,
        label: Value,
        value: Value,
    ) -> ActionResult<Value> {
        if matches!(label_type, Value::Sym("label")) {
            let name = label.token_str().to_owned();
            self.check_duplicate_pattern_key(&name, label.loc())?;
            Ok(self.b_pair_keyword(label, value))
        } else {
            let begin_t = label.at(0);
            let parts = label.at(1);
            let end_t = label.at(2);
            let label_loc = join_opt(begin_t.loc(), end_t.loc());
            match Self::static_string(parts.as_list()) {
                Some(name) => self.check_duplicate_pattern_key(&name, label_loc)?,
                None => self.builder_error(
                    "symbol literal with interpolation is not allowed",
                    label_loc,
                )?,
            }
            Ok(self.b_pair_quoted(begin_t, parts, end_t, value))
        }
    }

    pub(crate) fn b_match_label(&mut self, label_type: Value, label: Value) -> ActionResult<Value> {
        if matches!(label_type, Value::Sym("label")) {
            self.b_match_hash_var(label)
        } else {
            self.b_match_hash_var_from_str(label.at(0), label.at(1), label.at(2))
        }
    }

    // ----- Verification -----

    fn check_duplicate_args(
        &mut self,
        args: &[Value],
        map: &mut HashMap<Option<Rc<str>>, Node>,
    ) -> ActionResult<()> {
        for arg in args {
            let Some(node) = arg.as_node() else {
                continue;
            };
            match node.kind {
                "arg" | "optarg" | "restarg" | "blockarg" | "kwarg" | "kwoptarg" | "kwrestarg"
                | "shadowarg" => self.check_duplicate_arg(node, map)?,
                "procarg0" => {
                    if matches!(node.children.first(), Some(Value::Str(_))) {
                        self.check_duplicate_arg(node, map)?;
                    } else {
                        let children = node.children.clone();
                        self.check_duplicate_args(&children, map)?;
                    }
                }
                "mlhs" => {
                    let children = node.children.clone();
                    self.check_duplicate_args(&children, map)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn check_duplicate_arg(
        &mut self,
        this_arg: &Node,
        map: &mut HashMap<Option<Rc<str>>, Node>,
    ) -> ActionResult<()> {
        let this_name = match this_arg.children.first() {
            Some(Value::Str(name)) => Some(Rc::clone(name)),
            _ => None,
        };
        match map.get(&this_name) {
            None => {
                map.insert(this_name, this_arg.clone());
            }
            Some(that_arg) => {
                let that_name = match that_arg.children.first() {
                    Some(Value::Str(name)) => Some(Rc::clone(name)),
                    _ => None,
                };
                // 2.0 以降: `_` で始まる名前は重複してよい。
                if let Some(name) = &this_name
                    && !name.starts_with('_')
                    && this_name == that_name
                {
                    self.builder_error("duplicate argument name", this_arg.loc.name.clone())?;
                }
            }
        }
        Ok(())
    }

    fn validate_no_forward_arg_after_restarg(&mut self, args: &[Value]) -> ActionResult<()> {
        let mut restarg = None;
        let mut forward_arg = None;
        for arg in args {
            match arg.kind() {
                Some("restarg") => restarg = Some(arg.clone()),
                Some("forward_arg") => forward_arg = Some(arg.clone()),
                _ => {}
            }
        }
        if let (Some(forward_arg), Some(_)) = (forward_arg, restarg) {
            self.builder_error("... after rest argument", forward_arg.expression())?;
        }
        Ok(())
    }

    fn check_assignment_to_numparam(&mut self, name: &str, loc: Option<Span>) -> ActionResult<()> {
        if self.context().in_dynamic_block()
            && numparam_name(name)
            && self.max_numparam_stack.has_numparams()
        {
            self.builder_error(format!("cannot assign to numbered parameter {name}"), loc)?;
        }
        Ok(())
    }

    fn check_reserved_for_numparam(&mut self, name: &str, loc: Option<Span>) -> ActionResult<()> {
        if self.version_number() >= 30 && numparam_name(name) {
            self.builder_error(format!("{name} is reserved for numbered parameter"), loc)?;
        }
        Ok(())
    }

    /// `/\A[[[:lower:]]_][[[:alnum:]]_]*\z/`。
    fn check_lvar_name(&mut self, name: &str, loc: Option<Span>) -> ActionResult<()> {
        let mut chars = name.chars();
        let valid = chars
            .next()
            .is_some_and(|first| first.is_lowercase() || first == '_')
            && chars.all(|c| c.is_alphanumeric() || c == '_');
        if !valid {
            self.builder_error(
                format!("`{name}' is not allowed as a local variable name"),
                loc,
            )?;
        }
        Ok(())
    }

    fn check_duplicate_pattern_variable(
        &mut self,
        name: &str,
        loc: Option<Span>,
    ) -> ActionResult<()> {
        if name.starts_with('_') {
            return Ok(());
        }
        if self.pattern_variables.is_declared(name) {
            self.builder_error(format!("duplicate variable name {name}"), loc)?;
        }
        self.pattern_variables.declare(name);
        Ok(())
    }

    fn check_duplicate_pattern_key(&mut self, name: &str, loc: Option<Span>) -> ActionResult<()> {
        if self.pattern_hash_keys.is_declared(name) {
            self.builder_error(format!("duplicate hash pattern key {name}"), loc)?;
        }
        self.pattern_hash_keys.declare(name);
        Ok(())
    }

    fn validate_definee(&mut self, definee: &Value) -> ActionResult<()> {
        if matches!(
            definee.kind(),
            Some("int" | "str" | "dstr" | "sym" | "dsym" | "regexp" | "array" | "hash")
        ) {
            self.builder_error(
                "cannot define a singleton method for a literal",
                definee.expression(),
            )?;
        }
        Ok(())
    }
}
