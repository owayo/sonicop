//! 字句上のローカル変数の束縛から、grammar が引数の始まりと読んだ演算子のうち Ruby が二項演算
//! として読むものの位置を探す。
//!
//! grammar は束縛を知らないので、`count = 1; count +1` の `+1` を `count` という名前の
//! メソッドの引数として読む。Ruby の字句解析は、ローカル変数の後 (EXPR_END) の `+`・`-`・`*`・
//! `**`・`&`・`<<`・`/`・`%` を二項演算子、`[` を添字として読む。そのまま cop に渡すと、変数を
//! 使っていないと誤って報告し、`-A` が引数名を `_count` へ変えたり代入を消したりしたうえで
//! `count(+1)` と括弧を足し、実行できないコードを書き戻す。grammar は演算子の直前の空白が
//! 改ページ (U+000C) なら二項演算として読むので、その空白の位置を集めて
//! [`crate::parser::parse`] が読み直す。
//!
//! 束縛は評価の順ではなく字句の順に決まる。`x = 1 if x +1` の条件の `x` は本文の代入より後に
//! 読まれるのでローカル変数で、`x +1 if x = 1` の本文の `x` はメソッド呼び出し。代入の左辺も
//! 左から読み、`ary[x +1], x = 0, 1` の最初の `x` は後ろの代入先より前なのでメソッド呼び出し。
//! heredoc の本文は開始の直後に読まれる ([`crate::heredocs`]) ので、本文の補間で代入した名前は、
//! 同じ行のそれより後ろでも束縛済みになる。`def`・`class`・`module` は外側の変数を引き継がず、
//! ブロックとラムダは引き継ぐ。ブロックの中で初めて代入した名前はブロックの外へ残らない。
//!
//! 走査は明示したスタックで回す。rayon の worker のスタックは小さく、深く入れ子になった
//! ファイルで再帰するとプロセスごと落ちる。

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, TreeCursor};

use crate::rules::support::{scope_kind, spurious_assignment_list};

/// 二項演算として読み直す手がかり。原文のバイト位置の昇順で重複しない。
///
/// `hinted` は、`root` を読むときに置き換え・挿入した手がかり (原文の位置の昇順)。そこの演算子は
/// grammar がすでに二項演算として読んでいるので、直前の名前がその字句の位置でいまもローカル変数に
/// 束縛されているときだけ結果に残す。読み直しで束縛が消えた手がかり (heredoc だった本文がコメントに
/// 変わった、など) は落ちる。
pub(crate) fn binary_operator_hints(
    root: Node<'_>,
    text: &str,
    hinted: &[Hint],
    semantics: crate::regexp_ast::Semantics,
) -> Vec<Hint> {
    let (beginnings, bodies) = heredoc_nodes(root);
    let heredocs: HashMap<usize, Node<'_>> = crate::heredocs::pair(beginnings, bodies, text)
        .into_iter()
        .map(|(beginning, body)| (beginning.id(), body))
        .collect();
    let mut walk = Walk {
        text,
        hinted,
        semantics,
        frames: vec![Frame::new(false)],
        heredocs,
        cursor: root.walk(),
        hints: Vec::new(),
    };
    walk.run(root);
    let mut hints = walk.hints;
    hints.sort_unstable_by_key(|hint| hint.position());
    hints.dedup();
    hints
}

/// 原文の座標で表す手がかり。空白が無い行継続だけ、直前へ仮想の改ページを挿入する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hint {
    Replace(usize),
    Insert(usize),
}

impl Hint {
    pub(crate) fn position(self) -> usize {
        match self {
            Self::Replace(position) | Self::Insert(position) => position,
        }
    }
}

/// 1 つのスコープで、ここまでに宣言した名前。名前付きキャプチャの名前は本文の部分文字列とは
/// 限らない (行継続やエスケープを整えた名前) ので、持つこともできる。
struct Frame<'a> {
    /// ブロックとラムダは外側の変数を見られる。`def`・`class`・`module` と最上位は見られない。
    block: bool,
    names: HashSet<Cow<'a, str>>,
}

impl Frame<'_> {
    fn new(block: bool) -> Self {
        Self {
            block,
            names: HashSet::new(),
        }
    }
}

enum Step<'tree> {
    Visit(Node<'tree>),
    /// スコープの中身を読む前に、その枠を積む。
    Enter(bool),
    Leave,
    /// 仮引数の名前。既定値より先に宣言する。
    Declare(Node<'tree>),
    /// 代入の左辺。左から読み、代入先に着いた時点で宣言する。
    Target(Node<'tree>),
    /// パターンの中で束縛する名前と、パターンに書いた式。
    Pattern(Node<'tree>),
    /// `/(?<name>…)/ =~ text` が、照合を読み終えた後に宣言する名前。
    Captures(Node<'tree>),
}

struct Walk<'a, 'tree> {
    text: &'a str,
    semantics: crate::regexp_ast::Semantics,
    /// いまの木を読むときに置き換え・挿入した手がかり (原文の位置の昇順)。
    hinted: &'a [Hint],
    frames: Vec<Frame<'a>>,
    /// `<<~X` から、その本文。本文は開始の直後に読む。
    heredocs: HashMap<usize, Node<'tree>>,
    /// 子を並べるたびに作らないよう、1 つを使い回す。
    cursor: TreeCursor<'tree>,
    hints: Vec<Hint>,
}

impl<'a, 'tree> Walk<'a, 'tree> {
    fn run(&mut self, root: Node<'tree>) {
        let bodies: HashSet<usize> = self.heredocs.values().map(Node::id).collect();
        let mut stack = vec![Step::Visit(root)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Visit(node) => self.visit(node, &bodies, &mut stack),
                Step::Enter(block) => self.frames.push(Frame::new(block)),
                Step::Leave => {
                    self.frames.pop();
                }
                Step::Declare(name) => self.declare(name),
                Step::Target(target) => self.target(target, &mut stack),
                Step::Pattern(node) => self.pattern(node, &mut stack),
                Step::Captures(regex) => {
                    for name in self.semantics.named_groups(regex, self.text, |beginning| {
                        self.heredocs.get(&beginning.id()).copied()
                    }) {
                        self.declare_name(Cow::Owned(name));
                    }
                }
            }
        }
    }

    fn visit(&mut self, node: Node<'tree>, bodies: &HashSet<usize>, stack: &mut Vec<Step<'tree>>) {
        let kind = node.kind();
        if let Some((block, outer)) = scope_kind(kind) {
            // `class Foo < bar` の `bar` と `def obj.name` の `obj` は外側のスコープで読む。
            let outer: Vec<Node<'tree>> = outer
                .iter()
                .filter_map(|field| node.child_by_field_name(field))
                .collect();
            stack.push(Step::Leave);
            self.push_children(node, stack, |child| {
                !outer.iter().any(|field| field.id() == child.id())
            });
            stack.push(Step::Enter(block));
            for field in outer.into_iter().rev() {
                stack.push(Step::Visit(field));
            }
            return;
        }
        match kind {
            // Ruby は代入の左辺を読んだ時点で名前を登録するので、右辺では束縛済み (`x = x +1`)。
            "assignment" | "operator_assignment" if !mislexed_match_operator(node, self.text) => {
                if let Some(right) = node.child_by_field_name("right") {
                    stack.push(Step::Visit(right));
                }
                if let Some(left) = node.child_by_field_name("left") {
                    stack.push(Step::Target(left));
                }
            }
            "method_parameters" | "block_parameters" | "lambda_parameters" => {
                self.parameters(node, stack);
            }
            "exception_variable" => {
                if let Some(target) = node.named_child(0) {
                    stack.push(Step::Target(target));
                }
            }
            "for" => {
                let pattern = node.child_by_field_name("pattern");
                self.push_children(node, stack, |child| {
                    pattern.is_none_or(|pattern| pattern.id() != child.id())
                });
                if let Some(pattern) = pattern {
                    stack.push(Step::Target(pattern));
                }
            }
            "in_clause" => {
                let pattern = node.child_by_field_name("pattern");
                self.push_children(node, stack, |child| {
                    pattern.is_none_or(|pattern| pattern.id() != child.id())
                });
                if let Some(pattern) = pattern {
                    stack.push(Step::Pattern(pattern));
                }
            }
            // `value => pattern` と `value in pattern` は値を先に読む。
            "match_pattern" | "test_pattern" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    stack.push(Step::Pattern(pattern));
                }
                if let Some(value) = node.child_by_field_name("value") {
                    stack.push(Step::Visit(value));
                }
            }
            "binary" if self.declares_captures(node) => {
                let left = node.child_by_field_name("left").expect("確かめた");
                stack.push(Step::Captures(left));
                self.push_children(node, stack, |_| true);
            }
            "heredoc_beginning" => {
                if let Some(&body) = self.heredocs.get(&node.id()) {
                    self.push_children(body, stack, |_| true);
                }
            }
            // 開始の直後に読んだ。対応の取れない本文だけ、木の位置で読む。
            "heredoc_body" if bodies.contains(&node.id()) => {}
            "call" => {
                self.check_call(node);
                self.push_children(node, stack, |_| true);
            }
            "identifier" => self.check_hinted(node),
            _ => self.push_children(node, stack, |_| true),
        }
    }

    /// 名前付きの子を、先頭から読まれる順にスタックへ積む。
    fn push_children(
        &mut self,
        node: Node<'tree>,
        stack: &mut Vec<Step<'tree>>,
        keep: impl Fn(Node<'tree>) -> bool,
    ) {
        let start = stack.len();
        for child in self.named_children(node) {
            if keep(child) {
                stack.push(Step::Visit(child));
            }
        }
        stack[start..].reverse();
    }

    fn named_children(&mut self, node: Node<'tree>) -> Vec<Node<'tree>> {
        let mut children = Vec::new();
        self.cursor.reset(node);
        if self.cursor.goto_first_child() {
            loop {
                let child = self.cursor.node();
                if child.is_named() {
                    children.push(child);
                }
                if !self.cursor.goto_next_sibling() {
                    break;
                }
            }
        }
        children
    }

    fn parameters(&mut self, node: Node<'tree>, stack: &mut Vec<Step<'tree>>) {
        // 仮引数は左から順に登録され、既定値はそれより前の仮引数だけを見る。スタックは後ろから
        // 取り出すので、読む順の逆に積む。
        let mut steps = Vec::new();
        let mut pending = self.named_children(node);
        while let Some(parameter) = pending.pop() {
            match parameter.kind() {
                "identifier" => steps.push(Step::Declare(parameter)),
                // `|(a, b)|` は要素ごとに宣言する。
                "destructured_parameter" => pending.extend(self.named_children(parameter)),
                "optional_parameter"
                | "keyword_parameter"
                | "splat_parameter"
                | "hash_splat_parameter"
                | "block_parameter" => {
                    if let Some(value) = parameter.child_by_field_name("value") {
                        steps.push(Step::Visit(value));
                    }
                    if let Some(name) = parameter.child_by_field_name("name") {
                        steps.push(Step::Declare(name));
                    }
                }
                _ => steps.push(Step::Visit(parameter)),
            }
        }
        stack.extend(steps);
    }

    /// 代入の左辺を左から読み、代入先に着いた時点で宣言する。
    fn target(&mut self, target: Node<'tree>, stack: &mut Vec<Step<'tree>>) {
        match target.kind() {
            "identifier" => self.declare(target),
            // grammar が並びに飲み込んだ前の要素は、代入先ではなく同じ並びの式。
            "left_assignment_list" if spurious_assignment_list(target) => {
                let children = self.named_children(target);
                if let Some((&last, leading)) = children.split_last() {
                    stack.push(Step::Target(last));
                    for &child in leading.iter().rev() {
                        stack.push(Step::Visit(child));
                    }
                }
            }
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                for child in self.named_children(target).into_iter().rev() {
                    stack.push(Step::Target(child));
                }
            }
            // `ary[i] = …` と `obj.attr = …` は代入先の式を読む。
            _ => stack.push(Step::Visit(target)),
        }
    }

    /// パターンが束縛する名前を宣言し、パターンに書いた式を読む。
    fn pattern(&mut self, node: Node<'tree>, stack: &mut Vec<Step<'tree>>) {
        match node.kind() {
            "identifier" => self.declare(node),
            "array_pattern"
            | "find_pattern"
            | "hash_pattern"
            | "alternative_pattern"
            | "as_pattern"
            | "splat_parameter"
            | "hash_splat_parameter" => {
                for child in self.named_children(node).into_iter().rev() {
                    stack.push(Step::Pattern(child));
                }
            }
            // `in {name:}` は `name` を束縛し、`in {name: pattern}` は値のパターンが束縛する。
            "keyword_pattern" => match node.child_by_field_name("value") {
                Some(value) => stack.push(Step::Pattern(value)),
                None => {
                    if let Some(key) = node.child_by_field_name("key") {
                        let name = self.text[key.byte_range()].trim_end_matches(':');
                        self.declare_name(Cow::Borrowed(name));
                    }
                }
            },
            // `in ^name` は束縛ではなく読み取り。
            _ => stack.push(Step::Visit(node)),
        }
    }

    fn declare(&mut self, name: Node<'tree>) {
        if let Some(name) = self.text.get(name.byte_range()) {
            self.declare_name(Cow::Borrowed(name));
        }
    }

    fn declare_name(&mut self, name: Cow<'a, str>) {
        self.frames
            .last_mut()
            .expect("最上位の枠は残る")
            .names
            .insert(name);
    }

    /// その位置で名前がローカル変数に束縛されているか。
    fn is_local(&self, name: &str) -> bool {
        // `_1` から `_9` はブロックの番号付き引数で、宣言しなくてもローカル変数。
        let numbered = name.len() == 2
            && name.as_bytes()[0] == b'_'
            && matches!(name.as_bytes()[1], b'1'..=b'9');
        for frame in self.frames.iter().rev() {
            if frame.names.contains(name) || (numbered && frame.block) {
                return true;
            }
            if !frame.block {
                return false;
            }
        }
        false
    }

    /// grammar が `name +x` を `name` の呼び出しと読んだ位置で、`name` がローカル変数なら、
    /// 演算子の直前の空白を手がかりの位置として記録する。
    ///
    /// ブロックが付いていても束縛で決める。`i = 1; i +1 do end` は Ruby では `i + 1` に `do` が
    /// 続く構文エラーで、`x = []; x [0].each do end` は添字の後の呼び出しにブロックが付く。
    fn check_call(&mut self, call: Node<'tree>) {
        if call.child_by_field_name("receiver").is_some() {
            return;
        }
        let (Some(method), Some(arguments)) = (
            call.child_by_field_name("method"),
            call.child_by_field_name("arguments"),
        ) else {
            return;
        };
        if method.kind() != "identifier" || arguments.kind() != "argument_list" {
            return;
        }
        // `name(+x)` は括弧が名前に接しているので、ローカル変数があっても呼び出し。
        // 名前と引数の間は空白と行継続だけ。空白が無い場合は、行継続の直前へ挿入する。
        let bytes = self.text.as_bytes();
        let start = arguments.start_byte();
        if start <= method.end_byte() || !is_blank(&bytes[method.end_byte()..start]) {
            return;
        }
        let Some(hint) = hint_slot(bytes, method.end_byte(), start) else {
            return;
        };
        let available = match hint {
            Hint::Replace(position) => matches!(bytes[position], b' ' | b'\t'),
            Hint::Insert(_) => true,
        };
        if available
            && operator_starts(bytes, start)
            && self.is_local(&self.text[method.byte_range()])
        {
            self.hints.push(hint);
        }
    }

    /// 前に読んだときの手がかりで二項演算として読ませた位置を、いまの木で確かめ直す。直前の名前が
    /// この字句の位置でまだローカル変数なら、手がかりを残す。
    fn check_hinted(&mut self, name: Node<'tree>) {
        if self.hinted.is_empty() {
            return;
        }
        // `obj.name +1` の `name` はメソッド名で、ローカル変数と同じ名前でも引数を取る。
        if let Some(parent) = name.parent()
            && parent.kind() == "call"
            && parent.child_by_field_name("receiver").is_some()
            && parent
                .child_by_field_name("method")
                .is_some_and(|method| method.id() == name.id())
        {
            return;
        }
        let bytes = self.text.as_bytes();
        let start = gap_end(bytes, name.end_byte());
        let Some(hint) = hint_slot(bytes, name.end_byte(), start) else {
            return;
        };
        let available = match hint {
            Hint::Replace(position) => bytes[position] == b'\x0c',
            Hint::Insert(_) => true,
        };
        if available
            && self
                .hinted
                .binary_search_by_key(&hint.position(), |hint| hint.position())
                .is_ok_and(|index| self.hinted[index] == hint)
            && operator_starts(bytes, start)
            && self.is_local(&self.text[name.byte_range()])
        {
            self.hints.push(hint);
        }
    }

    /// 本家が静的に読む regexp だけを宣言候補にし、名前抽出で架空の束縛を除く。
    fn declares_captures(&self, node: Node<'tree>) -> bool {
        let Some(left) = node.child_by_field_name("left") else {
            return false;
        };
        let operator = node
            .child_by_field_name("operator")
            .map(|operator| &self.text[operator.byte_range()]);
        operator == Some("=~")
            && self.semantics.is_static(left, self.text, |beginning| {
                self.heredocs.get(&beginning.id()).copied()
            })
    }
}

/// Ruby の字句解析が、ローカル変数の後で二項演算子 (`[` は添字) として読む演算子の始まりか。
fn operator_starts(bytes: &[u8], start: usize) -> bool {
    match bytes.get(start) {
        Some(b'+' | b'-' | b'*' | b'&' | b'[' | b'/' | b'%') => true,
        Some(b'<') => bytes.get(start + 1) == Some(&b'<'),
        _ => false,
    }
}

/// 名前の終わり `name_end` から演算子 `operator` までの隙間で、手がかり (改ページ) を置く位置。
/// 隙間の末尾の行継続を飛ばした直前の空白・タブ・改ページで、grammar は行継続の前の手がかりを
/// 改行を越えて保つ (`count \` + 改行 + `+1`)。隙間が行継続だけなら、名前の直後へ挿入する。
fn hint_slot(bytes: &[u8], name_end: usize, operator: usize) -> Option<Hint> {
    let mut end = operator;
    loop {
        let gap = &bytes[name_end..end];
        if gap.ends_with(b"\\\n") {
            end -= 2;
        } else if gap.ends_with(b"\\\r\n") {
            end -= 3;
        } else {
            break;
        }
    }
    if end > name_end && matches!(bytes[end - 1], b' ' | b'\t' | b'\x0c') {
        Some(Hint::Replace(end - 1))
    } else if end == name_end && end < operator {
        // 隙間が行継続だけなら置き換えられる空白が無い。仮想の改ページは名前の直後へ置き、
        // parse の後に Tree::edit で消して、返す木の座標を原文へ戻す。
        Some(Hint::Insert(name_end))
    } else {
        None
    }
}

/// `from` から空白・タブ・改ページと行継続を読み飛ばした位置。
fn gap_end(bytes: &[u8], mut from: usize) -> usize {
    while let Some(&byte) = bytes.get(from) {
        match byte {
            b' ' | b'\t' | b'\x0c' => from += 1,
            b'\\' if bytes.get(from + 1) == Some(&b'\n') => from += 2,
            b'\\' if bytes.get(from + 1) == Some(&b'\r') && bytes.get(from + 2) == Some(&b'\n') => {
                from += 3;
            }
            _ => break,
        }
    }
    from
}

/// 空白・タブと行継続 (`\` と改行) だけでできているか。
fn is_blank(gap: &[u8]) -> bool {
    let mut index = 0;
    while index < gap.len() {
        match gap[index] {
            b' ' | b'\t' => index += 1,
            b'\\' => match gap.get(index + 1) {
                Some(b'\n') => index += 2,
                Some(b'\r') if gap.get(index + 2) == Some(&b'\n') => index += 3,
                _ => return false,
            },
            _ => return false,
        }
    }
    true
}

/// `f(nil, r =~ x)` は代入ではない。Ruby の字句解析は接した `=~` を 1 つのトークンにするが、
/// grammar は `=` と `~` に分けて、引数の並びから多重代入を作る。
fn mislexed_match_operator(node: Node<'_>, text: &str) -> bool {
    let Some(right) = node.child_by_field_name("right") else {
        return false;
    };
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| !child.is_named() && &text[child.byte_range()] == "=");
    found.is_some_and(|operator| {
        operator.end_byte() == right.start_byte() && text[right.byte_range()].starts_with('~')
    })
}

fn heredoc_nodes(root: Node<'_>) -> (Vec<Node<'_>>, Vec<Node<'_>>) {
    let mut beginnings = Vec::new();
    let mut bodies = Vec::new();
    let mut cursor = root.walk();
    'walk: loop {
        let node = cursor.node();
        match node.kind() {
            "heredoc_beginning" => beginnings.push(node),
            "heredoc_body" => bodies.push(node),
            _ => {}
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    (beginnings, bodies)
}

#[cfg(test)]
mod tests {
    use super::{Hint, binary_operator_hints};

    fn tree(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    /// 手がかりを置く空白ごとに、直前の識別子から演算子の 1 文字目までを返す (`"count +"`)。
    fn hinted(source: &str) -> Vec<String> {
        let tree = tree(source);
        binary_operator_hints(
            tree.root_node(),
            source,
            &[],
            crate::regexp_ast::Semantics::default(),
        )
        .into_iter()
        .map(|hint| {
            let position = hint.position();
            let before = source[..position].trim_end_matches([' ', '\t', '\n', '\r', '\\']);
            let start = before
                .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                .map_or(0, |index| index + 1);
            let after = source[position + usize::from(matches!(hint, Hint::Replace(_)))..]
                .trim_start_matches(['\\', '\r', '\n']);
            format!("{} {}", &before[start..], &after[..1])
        })
        .collect()
    }

    /// 仮引数・代入・多重代入・`for`・`rescue`・パターン・名前付きキャプチャ・番号付き引数の
    /// どれで宣言した名前も、後ろの演算子を二項演算にする。
    #[test]
    fn every_kind_of_declaration_binds_the_name() {
        for (source, expected) in [
            ("[1].map { |i| i +1 }\n", vec!["i +"]),
            ("def m(count)\n  count -1\nend\n", vec!["count -"]),
            ("total = 1\ntotal +1\n", vec!["total +"]),
            ("a, b = 1, 2\nb *a\n", vec!["b *"]),
            ("x ||= 1\nx &y\n", vec!["x &"]),
            ("x = 1\nx **2\n", vec!["x *"]),
            ("for item in list\n  item [0]\nend\n", vec!["item ["]),
            ("begin\nrescue => error\n  error +1\nend\n", vec!["error +"]),
            ("log = []\nlog <<EOS\nEOS\n", vec!["log <"]),
            (
                "case value\nin [first, *rest]\n  first +rest.size\nend\n",
                vec!["first +"],
            ),
            (
                "case value\nin {name:}\n  name %w[a]\nend\n",
                vec!["name %"],
            ),
            ("value => found\nfound /x/\n", vec!["found /"]),
            ("/(?<year>\\d+)/ =~ text\nyear +1\n", vec!["year +"]),
            ("%r{(?<year>\\d+)}x =~ text\nyear +1\n", vec!["year +"]),
            // Ruby は閉じの区切り文字のエスケープを外してから Onigmo に渡す。
            ("%r'(?\\'name\\'a)' =~ text\nname +1\n", vec!["name +"]),
            // `\c[` などは制御文字・メタ文字 1 つで、`[` は文字クラスを開かない。
            ("/\\c[(?<real>a)/ =~ text\nreal +1\n", vec!["real +"]),
            ("/\\C-[(?<real>a)/ =~ text\nreal +1\n", vec!["real +"]),
            ("/\\M-\\C-[(?<real>a)/n =~ text\nreal +1\n", vec!["real +"]),
            ("/\\c\\M-[(?<real>a)/n =~ text\nreal +1\n", vec!["real +"]),
            ("/[\\c?](?<real>a)/ =~ text\nreal +1\n", vec!["real +"]),
            ("[1].map { _1 +1 }\n", vec!["_1 +"]),
            ("[1].each { |x; y| y = 1; y +x }\n", vec!["y +"]),
            ("x = 1\nx \\\n  +1\n", vec!["x +"]),
            // 演算子が行継続の直後なら、行継続の前の空白に置く。
            ("x = 1\nx \\\n+1\n", vec!["x +"]),
            ("x = 1\nx\\\n+1\n", vec!["x +"]),
            ("x = 1\r\nx\\\r\n+1\r\n", vec!["x +"]),
            ("x = 1\r\nx \\\r\n+1\r\n", vec!["x +"]),
            // ブロックが付いていても、ローカル変数の後の演算子は二項演算 (`do` は構文エラー)。
            ("i = 1\ni +1 do\nend\n", vec!["i +"]),
            ("x = []\nx [0].each do |y| y end\n", vec!["x ["]),
        ] {
            assert_eq!(hinted(source), expected, "{source:?}");
        }
    }

    /// 束縛は字句の順に決まる。修飾子の本文は条件より先に、代入の左辺は左から読まれ、heredoc の
    /// 本文は開始の直後に読まれる。
    #[test]
    fn bindings_follow_the_order_ruby_reads_the_source_in() {
        for (source, expected) in [
            ("x = 1 if x +1\n", vec!["x +"]),
            ("x +1 if x = 1\n", vec![]),
            ("x +1\nx = 1\n", vec![]),
            ("ary[x +1], x = 0, 1\n", vec![]),
            ("foo(x +1, x = 2)\n", vec![]),
            ("foo(<<~A, x +1)\n  #{x = 1}\nA\n", vec!["x +"]),
            // 開始は A・B・C の順、本文は A・C・B の順に読まれる。C の本文の `x` は、B の本文の
            // 代入より前に読まれるのでメソッド呼び出し。
            (
                "puts(<<~A, <<~B)\n  #{puts(<<~C)}\n    #{x +1}\n  C\nA\n  #{x = 1}\nB\n",
                vec![],
            ),
        ] {
            assert_eq!(hinted(source), expected, "{source:?}");
        }
    }

    /// `def`・`class` は外側の変数を見ず、ブロックとラムダは見る。ブロックで初めて代入した名前は
    /// 外へ残らない。
    #[test]
    fn scopes_decide_which_names_are_visible() {
        for (source, expected) in [
            ("x = 1\ndef m\n  x +1\nend\n", vec![]),
            ("x = 1\nclass Foo\n  x +1\nend\n", vec![]),
            ("x = 1\n[1].each { x +1 }\n", vec!["x +"]),
            ("x = 1\n-> { x +1 }\n", vec!["x +"]),
            ("[1].each { y = 1 }\ny +1\n", vec![]),
            ("_1 +1\n", vec![]),
        ] {
            assert_eq!(hinted(source), expected, "{source:?}");
        }
    }

    /// 名前付きキャプチャに見えても、文字クラスやコメントの中の `(?<…>)` は変数を作らない。
    /// 条件付きの部分式のように、名前の走査が意味を問わない構文が混じっても同じ。
    #[test]
    fn only_real_named_groups_declare_locals() {
        for source in [
            "/[(?<ghost>)]/ =~ text\nghost +1\n",
            "/a(?#(?<ghost>)/ =~ text\nghost +1\n",
            "/a # (?<ghost>)\n/x =~ text\nghost +1\n",
            "/(?<ghost>#{x})/ =~ text\nghost +1\n",
            "/(a)(?(1)b|c)[(?<ghost>)]/ =~ text\nghost +1\n",
            // `\c]` などは制御文字・メタ文字 1 つで、`]` は文字クラスを閉じない。
            "/[\\c](?<ghost>)]/ =~ text\nghost +1\n",
            "/[\\M-](?<ghost>)]/n =~ text\nghost +1\n",
        ] {
            assert_eq!(hinted(source), Vec::<String>::new(), "{source:?}");
        }
    }

    /// ローカル変数と同じ名前でも、括弧が接した呼び出し・レシーバ付きの呼び出しと、すでに
    /// 二項演算と読まれた形には手がかりを置かない。
    /// ブロックの暗黙の引数 `it` (Ruby 3.4 以降) は、一度参照した後でも字句解析はローカル変数として
    /// 扱わない (Prism と parse.y のどちらでも `it +1` は `it(+1)`)。
    #[test]
    fn calls_ruby_reads_as_calls_stay_calls() {
        for source in [
            "i = 1\ni(+1)\n",
            "i = 1\ni(+1) do end\n",
            "i = 1\nself.i +1\n",
            "puts +1\n",
            "i = 1\ni + 1\n",
            "[1].map { it +1 }\n",
            "[1].map { it; it +1 }\n",
        ] {
            assert_eq!(hinted(source), Vec::<String>::new(), "{source:?}");
        }
    }

    /// 前に読んだときの手がかり (改ページ) は、いまの木で直前の名前が束縛されているときだけ残る。
    #[test]
    fn hints_from_an_earlier_reading_are_checked_again() {
        for (source, kept) in [
            ("x = 1\nx\x0c+1\n", true),
            ("x\x0c+1\n", false),
            ("x = 1\nobj.x\x0c+1\n", false),
            ("x = 1\nx \\\n\x0c+1\n", true),
            ("x = 1\nx\x0c\\\n+1\n", true),
            ("item = []\nitem\x0c[0]\n", true),
        ] {
            let tree = tree(source);
            let position = source.find('\x0c').unwrap();
            let hints = binary_operator_hints(
                tree.root_node(),
                source,
                &[Hint::Replace(position)],
                crate::regexp_ast::Semantics::default(),
            );
            assert_eq!(
                hints.contains(&Hint::Replace(position)),
                kept,
                "{source:?} {hints:?}"
            );
        }
    }

    /// 行継続の前後に置き換えられる空白があれば使い、行継続だけの隙間で初めて挿入する。
    #[test]
    fn hints_prefer_replacing_whitespace_to_inserting() {
        for (source, insert) in [
            ("x = 1\nx \\\n+1\n", false),
            ("x = 1\nx\\\n +1\n", false),
            ("x = 1\nx\\\n+1\n", true),
            ("x = 1\r\nx\\\r\n\\\n+1\n", true),
        ] {
            let tree = tree(source);
            let hints = binary_operator_hints(
                tree.root_node(),
                source,
                &[],
                crate::regexp_ast::Semantics::default(),
            );
            assert_eq!(hints.len(), 1, "{source:?} {hints:?}");
            assert_eq!(matches!(hints[0], Hint::Insert(_)), insert, "{source:?}");
        }
    }
}
