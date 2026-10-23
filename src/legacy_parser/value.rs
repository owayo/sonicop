//! racc の値スタックに積まれる値。parser gem の文法動作は Ruby の配列・トークン・AST ノードを
//! そのまま受け渡すので、動作を移すのに必要な形をここで表す。

use std::ops::Range;
use std::rc::Rc;

use super::tables::tokens::Tok;
use crate::engine::LiteralEncoding;

/// 文字単位の位置。parser gem は字句解析器の `source_pts` (コードポイントの配列) の添字で位置を持つ。
pub(crate) type Span = Range<usize>;

/// 字句解析器が出すトークン (`[type, [value, range]]`)。
#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub(crate) tok: Tok,
    pub(crate) value: TokenValue,
    pub(crate) range: Span,
}

/// トークンの値。数値の値は診断に使わないので、字句をそのまま持つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenValue {
    /// `nil` (`tNL`・`tSPACE` など)。builder の `loc` はこのトークンの位置を `nil` にする。
    Nil,
    Str(Rc<str>),
    /// 復号した文字列は UTF-8 に変換せず、Ruby のバイト列と符号化を保つ。
    Literal {
        bytes: Rc<[u8]>,
        encoding: LiteralEncoding,
    },
    /// `call_op` が作る `[:dot, range]` / `[:anddot, range]`。
    Sym(&'static str),
}

impl Token {
    pub(crate) fn str(&self) -> &str {
        match &self.value {
            TokenValue::Str(value) => value,
            TokenValue::Sym(value) => value,
            // 識別子の字句にはこの値を使わない。不正な UTF-8 を置換して名前にしない。
            TokenValue::Literal { bytes, .. } => std::str::from_utf8(bytes).unwrap_or_default(),
            TokenValue::Nil => "",
        }
    }
}

/// parser gem の文脈 (`Parser::Context`)。文法動作は `@context.dup` を値として持ち回る。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub(crate) in_defined: bool,
    pub(crate) in_kwarg: bool,
    pub(crate) in_argdef: bool,
    pub(crate) in_def: bool,
    pub(crate) in_class: bool,
    pub(crate) in_block: bool,
    pub(crate) in_lambda: bool,
    pub(crate) cant_return: bool,
}

impl Context {
    pub(crate) fn in_dynamic_block(&self) -> bool {
        self.in_block || self.in_lambda
    }
}

/// AST ノードの位置 (`Source::Map` のうち、診断が参照するもの)。
#[derive(Clone, Debug, Default)]
pub(crate) struct Loc {
    pub(crate) expression: Option<Span>,
    pub(crate) name: Option<Span>,
    pub(crate) keyword: Option<Span>,
    pub(crate) begin: Option<Span>,
    pub(crate) end: Option<Span>,
}

/// AST ノード。種類は parser gem の型名をそのまま使う。
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) kind: &'static str,
    pub(crate) children: Vec<Value>,
    pub(crate) loc: Loc,
}

// rayon の小さいスタックでも、深い式と回復で捨てた部分木を再帰せず解放する。
impl Drop for Node {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.children);
        while let Some(value) = pending.pop() {
            match value {
                Value::Node(node) => {
                    if let Ok(mut node) = Rc::try_unwrap(node) {
                        pending.append(&mut node.children);
                    }
                }
                Value::List(mut values) => pending.append(&mut values),
                _ => {}
            }
        }
    }
}

impl Node {
    pub(crate) fn expression(&self) -> Option<Span> {
        self.loc.expression.clone()
    }

    /// `node.updated(kind)`。
    pub(crate) fn updated(&self, kind: &'static str) -> Node {
        Node {
            kind,
            children: self.children.clone(),
            loc: self.loc.clone(),
        }
    }

    /// 最初の子が名前 (`Symbol`) のノードの名前。
    pub(crate) fn name(&self) -> Option<&str> {
        match self.children.first() {
            Some(Value::Str(name)) => Some(name),
            Some(Value::Literal { bytes, .. }) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }
}

/// 値スタックの値。Ruby の動作が扱う値の形に合わせる。
#[derive(Clone, Debug, Default)]
pub(crate) enum Value {
    #[default]
    Nil,
    Bool(bool),
    // 数値の値自体は診断で読まないが、文法動作の値を落とさず保持する。
    #[allow(dead_code)]
    Int(i64),
    Sym(&'static str),
    Str(Rc<str>),
    /// 同じバイトでも符号化が違うと名前は別物になるため、静的補間まで両方を保つ。
    Literal {
        bytes: Rc<[u8]>,
        encoding: LiteralEncoding,
    },
    Token(Rc<Token>),
    Node(Rc<Node>),
    List(Vec<Value>),
    Context(Context),
}

impl Value {
    pub(crate) fn node(node: Node) -> Self {
        Value::Node(Rc::new(node))
    }

    pub(crate) fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Ruby の真偽 (`nil` と `false` だけが偽)。
    pub(crate) fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub(crate) fn as_node(&self) -> Option<&Node> {
        match self {
            Value::Node(node) => Some(node),
            _ => None,
        }
    }

    pub(crate) fn as_token(&self) -> Option<&Token> {
        match self {
            Value::Token(token) => Some(token),
            _ => None,
        }
    }

    pub(crate) fn kind(&self) -> Option<&'static str> {
        self.as_node().map(|node| node.kind)
    }

    /// 配列として読む。`nil` は空の配列として扱わず、呼び出し側が区別する。
    pub(crate) fn into_list(self) -> Vec<Value> {
        match self {
            Value::List(items) => items,
            Value::Nil => Vec::new(),
            other => vec![other],
        }
    }

    pub(crate) fn as_list(&self) -> &[Value] {
        match self {
            Value::List(items) => items,
            _ => &[],
        }
    }

    /// `value[index]` (配列の要素)。範囲外と配列でないものは `nil`。
    pub(crate) fn at(&self, index: usize) -> Value {
        match self {
            Value::List(items) => items.get(index).cloned().unwrap_or_default(),
            _ => Value::Nil,
        }
    }

    pub(crate) fn as_context(&self) -> Context {
        match self {
            Value::Context(context) => *context,
            _ => Context::default(),
        }
    }

    /// builder の `loc(token)`: トークンの位置。`nil` やトークンでないもの、値が `nil` の
    /// トークン (`tNL`) は `nil`。
    pub(crate) fn loc(&self) -> Option<Span> {
        match self {
            Value::Token(token) if token.value != TokenValue::Nil => Some(token.range.clone()),
            _ => None,
        }
    }

    /// `node.loc.expression`。
    pub(crate) fn expression(&self) -> Option<Span> {
        self.as_node().and_then(Node::expression)
    }

    /// builder の `value(token)`。
    pub(crate) fn token_str(&self) -> &str {
        self.as_token().map_or("", Token::str)
    }
}

/// `a.join(b)`: 2 つの範囲を覆う範囲。
pub(crate) fn join(left: &Span, right: &Span) -> Span {
    left.start.min(right.start)..left.end.max(right.end)
}

/// `join` の片方が無いことを許す版。両方無ければ `None`。
pub(crate) fn join_opt(left: Option<Span>, right: Option<Span>) -> Option<Span> {
    match (left, right) {
        (Some(left), Some(right)) => Some(join(&left, &right)),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}
