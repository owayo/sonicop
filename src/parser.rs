//! One Ruby parser per worker thread.
//!
//! `Parser::new` allocates the parse stack, the lexer buffers and the reusable subtree pool, and
//! `set_language` resets all of them. That is work the file being read has nothing to do with, and
//! several places used to repeat it for every file, every recovered fragment and every syntax
//! probe. A sampling profile put it among the costs of a run.
//!
//! The tree a parse returns owns its own storage, so nothing here is shared with the caller.
//!
//! grammar はローカル変数の束縛を知らないので、`count = 1; count +1` の `+1` を引数として読む。
//! [`crate::lexical_locals`] が字句上の束縛から Ruby が二項演算として読む位置を探し、その直前の
//! 空白を改ページに置き換えて読み直す。名前に行継続が接して空白が無い場合だけ、その直前へ仮想の
//! 改ページを挿入し、parse の直後に Tree::edit で消す。返す木のバイト範囲と行・列は、どちらの
//! 経路でも呼び出し側が持つ原文に対応する。
//!
//! 改ページは grammar への手がかりなので、本文にもともとある改ページのうち、字句の空白にあるもの
//! (Ruby も空白として読む) は空白に置き換えて読む。文字列・heredoc の終端語・コメントのように
//! トークンの中にある改ページは文字そのものなので残す (`<<'A\fB'` の終端語を変えると本文の
//! 終わりが変わる)。
//!
//! どちらも読み方しだいで変わる。`x <<EOS` の `<<` を手がかりでシフトと読むと、heredoc だった
//! 本文はコードやコメントになり、そこにあった代入も改ページも意味が変わる。そこで「手がかりの
//! 位置の集合」と「空白にする元の改ページの集合」を状態にして、読むたびに両方をいまの木から
//! 計算し直し、変わらなくなるまで繰り返す。

use std::borrow::Cow;
use std::cell::RefCell;

use tree_sitter::{InputEdit, Parser, Point, Tree};

use crate::lexical_locals::Hint;

thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Parses `text` with this thread's parser, building one on first use.
///
/// `None` means the parser produced no tree at all, which is not the same as a tree holding
/// errors: a source the grammar cannot read still comes back as a tree with `ERROR` nodes.
pub(crate) fn parse(text: &str) -> Option<Tree> {
    parse_with_semantics(text, crate::regexp_ast::Semantics::default())
}

/// 検査と補正後の再検査では、選択した engine と版の正規表現の束縛規則を使う。
pub(crate) fn parse_configured(
    text: &str,
    semantics: crate::regexp_ast::Semantics,
) -> Option<Tree> {
    if semantics == crate::regexp_ast::Semantics::default() {
        return parse(text);
    }
    parse_with_semantics(text, semantics)
}

fn parse_with_semantics(text: &str, semantics: crate::regexp_ast::Semantics) -> Option<Tree> {
    let form_feeds: Vec<usize> = text
        .bytes()
        .enumerate()
        .filter_map(|(position, byte)| (byte == b'\x0c').then_some(position))
        .collect();
    // percent literal の中の式のように、読み直して初めて呼び出しに見える位置もあるので、
    // 状態が変わらなくなるまで回す。
    settle(State::default(), |state| {
        let readable = state.apply(text);
        // 読み直した木に構文エラーがあっても、束縛の判定に従った読み方を使う。
        // `x = 1; x +1, 2` は Ruby でも構文エラーで、手がかりのない木はそれを隠す。
        let mut tree = parse_raw(readable.text())?;
        readable.restore_coordinates(&mut tree);
        let next = State {
            blanked: whitespace_form_feeds(&tree, &form_feeds),
            hints: crate::lexical_locals::binary_operator_hints(
                tree.root_node(),
                &readable.analysis,
                &state.hints,
                semantics,
            ),
        };
        Some((tree, next))
    })
}

/// `step` が状態から結果と次の状態を作るのを、状態が変わらなくなるまで繰り返し、最後の結果を返す。
///
/// 前に覚えた状態へ戻ったら、それより先は同じ並びを回るだけなので、その回の結果で止める。循環は
/// Brent の方法で見つけ、覚える状態を 1 つに抑える (1・3・7… 回目の遷移の後の状態を覚え直す)。
fn settle<S: Clone + Eq, T>(mut state: S, mut step: impl FnMut(&S) -> Option<(T, S)>) -> Option<T> {
    let mut saved = state.clone();
    let mut power = 1usize;
    let mut steps = 0usize;
    loop {
        let (result, next) = step(&state)?;
        if next == state || next == saved {
            return Some(result);
        }
        steps += 1;
        if steps == power {
            saved = next.clone();
            power *= 2;
            steps = 0;
        }
        state = next;
    }
}

/// 読み直すたびに計算し直す、本文の置き換え方。どちらも原文のバイト位置の昇順。
#[derive(Clone, Default, PartialEq, Eq)]
struct State {
    /// 空白に置き換える、元の本文の改ページの位置。
    blanked: Vec<usize>,
    /// 二項演算を伝える手がかり。置き換えは `blanked` と重なれば手がかりが勝つ。
    hints: Vec<Hint>,
}

impl State {
    fn apply<'a>(&self, text: &'a str) -> Readable<'a> {
        let replaces = self
            .hints
            .iter()
            .any(|hint| matches!(hint, Hint::Replace(_)));
        let analysis = if self.blanked.is_empty() && !replaces {
            Cow::Borrowed(text)
        } else {
            let mut bytes = text.as_bytes().to_vec();
            for &position in &self.blanked {
                bytes[position] = b' ';
            }
            for &hint in &self.hints {
                if let Hint::Replace(position) = hint {
                    bytes[position] = b'\x0c';
                }
            }
            Cow::Owned(String::from_utf8(bytes).expect("ASCII を ASCII に置き換えた"))
        };
        let count = self
            .hints
            .iter()
            .filter(|hint| matches!(hint, Hint::Insert(_)))
            .count();
        let mut deletions = Vec::with_capacity(count);
        let inserted = (count != 0).then(|| {
            let mut readable = String::with_capacity(analysis.len() + count);
            let mut from = 0;
            let mut point = Point::new(0, 0);
            for &hint in &self.hints {
                let Hint::Insert(position) = hint else {
                    continue;
                };
                let part = &analysis[from..position];
                readable.push_str(part);
                // 前の挿入位置から進めるので、複数の手がかりでも本文を重複して走査しない。
                for byte in part.bytes() {
                    if byte == b'\n' {
                        point.row += 1;
                        point.column = 0;
                    } else {
                        point.column += 1;
                    }
                }
                let start_byte = readable.len();
                readable.push('\x0c');
                deletions.push(InputEdit {
                    start_byte,
                    old_end_byte: start_byte + 1,
                    new_end_byte: start_byte,
                    start_position: point,
                    old_end_position: Point::new(point.row, point.column + 1),
                    new_end_position: point,
                });
                point.column += 1;
                from = position;
            }
            readable.push_str(&analysis[from..]);
            readable
        });
        Readable {
            analysis,
            inserted,
            deletions,
        }
    }
}

/// `analysis` は原文と同じ長さ・改行位置で、束縛と元の改ページの判定に使う。挿入が必要なときだけ、
/// parser に渡す別の本文を持つ。通常の経路では原文を借りるだけ。
struct Readable<'a> {
    analysis: Cow<'a, str>,
    inserted: Option<String>,
    deletions: Vec<InputEdit>,
}

impl Readable<'_> {
    fn text(&self) -> &str {
        self.inserted.as_deref().unwrap_or(&self.analysis)
    }

    fn restore_coordinates(&self, tree: &mut Tree) {
        // 末尾から消せば、まだ適用していない前の位置や列は変わらない。編集後に取り出すノードは
        // すべて原文の座標を返す。以後の解析に、挿入した改ページの座標を持ち込まない。
        for deletion in self.deletions.iter().rev() {
            tree.edit(deletion);
        }
    }
}

/// `form_feeds` (元の本文の改ページ) のうち、`tree` の葉 (トークン) の外にあるもの。
fn whitespace_form_feeds(tree: &Tree, form_feeds: &[usize]) -> Vec<usize> {
    if form_feeds.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        if node.child_count() == 0 {
            tokens.push(node.byte_range());
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
    tokens.sort_by_key(|range| range.start);
    form_feeds
        .iter()
        .copied()
        .filter(|&position| {
            let next = tokens.partition_point(|range| range.end <= position);
            !tokens
                .get(next)
                .is_some_and(|range| range.start <= position)
        })
        .collect()
}

fn parse_raw(text: &str) -> Option<Tree> {
    PARSER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let parser = match slot.as_mut() {
            Some(parser) => parser,
            None => {
                let mut parser = Parser::new();
                parser
                    .set_language(&tree_sitter_ruby::LANGUAGE.into())
                    .expect("the grammar the whole run is parsed with");
                slot.insert(parser)
            }
        };
        parser.parse(text, None)
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// 束縛済みの名前の後の演算子は二項演算として読まれ、範囲は呼び出し側の本文のまま。
    #[test]
    fn a_local_variable_followed_by_an_operator_reads_as_a_binary_expression() {
        let text = "count = 1\ncount +1\n";
        let tree = parse(text).unwrap();
        let statement = tree.root_node().named_child(1).unwrap();
        assert_eq!(statement.kind(), "binary", "{}", tree.root_node().to_sexp());
        assert_eq!(&text[statement.byte_range()], "count +1");
    }

    fn kinds(tree: &tree_sitter::Tree) -> Vec<String> {
        let mut kinds = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            kinds.push(node.kind().to_owned());
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        kinds
    }

    /// percent literal の中の式は、外側を読み直して初めて呼び出しに見える。新しい位置が出なく
    /// なるまで読み直すので、何段あっても剰余と加算になる。
    #[test]
    fn nested_percent_literals_resolve_to_the_innermost_operator() {
        let text = "x = 1\nx %(x %(x %(x %(x +1))))\n";
        let tree = parse(text).unwrap();
        let kinds = kinds(&tree);
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );
        assert!(
            !kinds.iter().any(|kind| kind == "string" || kind == "call"),
            "{}",
            tree.root_node().to_sexp()
        );
    }

    /// ローカル変数の後の `+1, 2` と、`+1` に続く `do` は Ruby でも構文エラー。手がかりのない
    /// 読み方へ戻して隠さない。
    #[test]
    fn a_local_variable_does_not_take_command_arguments() {
        assert!(parse("x = 1\nx +1, 2\n").unwrap().root_node().has_error());
        assert!(
            parse("x = 1\nx +1 do\nend\n")
                .unwrap()
                .root_node()
                .has_error()
        );
    }

    /// 演算子が行継続の直後でも、行継続の前の空白に置いた手がかりで二項演算として読む。行継続が
    /// 続いても、途中に空白・タブや CRLF が混じっても同じ。
    #[test]
    fn a_hint_carries_across_a_line_continuation() {
        for text in [
            "count = 1\ncount \\\n+1\n",
            "count = 1\r\ncount \\\r\n+1\r\n",
            "count = 1\ncount \t\\\r\n\\\n+1\n",
            "count = 1\ncount \\\n \\\n+1\n",
            "count = 1\r\ncount\t\\\r\n\t\\\r\n-1\r\n",
        ] {
            let tree = parse(text).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{text:?}: {}",
                tree.root_node().to_sexp()
            );
            let snippet = &text[text.rfind("count").unwrap()..text.rfind('1').unwrap() + 1];
            assert_eq!(kind_of(text, snippet), "binary", "{text:?}");
        }
    }

    /// 添字の後の呼び出しにブロックが付く形は、手がかりで読み直した木でもブロックがその呼び出しに
    /// 残る。
    #[test]
    fn an_element_reference_keeps_the_block_on_the_following_call() {
        let text = "x = []\nx [0].each do |y| y end\n";
        let tree = parse(text).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{}", root.to_sexp());
        let statement = root.named_child(1).unwrap();
        assert_eq!(statement.kind(), "call", "{}", root.to_sexp());
        assert_eq!(
            statement.child_by_field_name("receiver").unwrap().kind(),
            "element_reference",
            "{}",
            root.to_sexp()
        );
        assert!(
            statement.child_by_field_name("block").is_some(),
            "{}",
            root.to_sexp()
        );
    }

    /// 遷移の表 (`next[i]` は状態 i の次の状態) を `settle` に渡し、止まった回の状態と、`step` を
    /// 呼んだ回数を返す。
    fn settled(next: &[usize]) -> (usize, usize) {
        let mut calls = 0;
        let result = super::settle(0usize, |&state| {
            calls += 1;
            Some((state, next[state]))
        });
        (result.unwrap(), calls)
    }

    /// 不動点ではその回の結果で止まる。循環に入ったら、前に覚えた状態へ戻った回で止まる。
    #[test]
    fn settling_stops_at_a_fixed_point_or_a_cycle() {
        // 不動点: 0 → 1 → 2 → 2
        assert_eq!(settled(&[1, 2, 2]), (2, 3));
        assert_eq!(settled(&[0]), (0, 1));
        // 前に 2 つ挟む 2 周期: 0 → 1 → (2 → 3)
        assert_eq!(settled(&[1, 2, 3, 2]), (2, 5));
        // 前に 1 つ挟む 3 周期: 0 → (1 → 2 → 3)
        assert_eq!(settled(&[1, 2, 3, 1]), (2, 6));
        // 覚え直した直後の状態へ戻る 2 周期: (0 → 1)
        assert_eq!(settled(&[1, 0]), (0, 3));
        // 前に 3 つ挟む 5 周期は、覚え直しを 3 回経て見つかる: 0 → 1 → 2 → (3 → … → 7)
        assert_eq!(settled(&[1, 2, 3, 4, 5, 6, 7, 3]), (6, 12));
        // `step` が結果を作れなければ、そこで止まる。
        assert_eq!(super::settle(0usize, |_| None::<(usize, usize)>), None);
    }

    /// 引用付きの heredoc の終端語の中の改ページは文字そのもの。空白に変えると、本文の
    /// `A B` の行で閉じてしまう。
    #[test]
    fn a_form_feed_inside_a_token_is_kept() {
        let text = "puts <<'A\x0cB'\nA B\nA\x0cB\n";
        let tree = parse(text).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{}", root.to_sexp());
        let mut stack = vec![root];
        let mut body = None;
        while let Some(node) = stack.pop() {
            if node.kind() == "heredoc_body" {
                body = Some(node);
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        let body = body.expect("heredoc の本文");
        assert!(
            text[body.byte_range()].contains("A B\n"),
            "{:?}",
            &text[body.byte_range()]
        );
    }

    /// 本文の中で `snippet` とちょうど同じ範囲を覆う、いちばん外側の名前付きノードの種類。
    fn kind_of(text: &str, snippet: &str) -> String {
        let tree = parse(text).unwrap();
        let start = text.find(snippet).expect("本文にある");
        let range = start..start + snippet.len();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if node.byte_range() == range {
                return node.kind().to_owned();
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        panic!(
            "{snippet:?} を覆うノードが無い: {}",
            tree.root_node().to_sexp()
        );
    }

    /// `<<` を手がかりでシフトと読み直すと、heredoc だった本文の代入はコメントになる。その代入に
    /// 頼って置いた手がかりは、読み直した後で外す。
    #[test]
    fn a_hint_whose_binding_disappears_is_withdrawn() {
        let text = "x = []\nx <<EOS\n#{y = 1}\nEOS\ny +1\n";
        assert_eq!(kind_of(text, "x <<EOS"), "binary");
        assert_eq!(kind_of(text, "y +1"), "call");
    }

    /// 元の改ページを空白にするかどうかも、読み直すたびにいまの木で決める。heredoc の本文だった
    /// 改ページが、`<<` をシフトと読んだ後にコードの空白になれば、手がかりとしては扱わない。
    #[test]
    fn form_feeds_are_classified_against_the_latest_reading() {
        let unbound = "x = []\nx <<EOS\ny\x0c+1\nEOS\n";
        assert_eq!(kind_of(unbound, "x <<EOS"), "binary");
        assert_eq!(kind_of(unbound, "y\x0c+1"), "call");
        let bound = "y = 1\nx = []\nx <<EOS\ny\x0c+1\nEOS\n";
        assert_eq!(kind_of(bound, "y\x0c+1"), "binary");
    }

    /// 終端語の中の改ページは文字のまま残し、同じ行のコードの改ページは空白として読む。
    #[test]
    fn form_feeds_in_a_terminator_and_in_code_are_told_apart() {
        for (text, operation, kind) in [
            (
                "x = 1\nputs <<'A\x0cB'; x\x0c+1\nA B\nA\x0cB\n",
                "x\x0c+1",
                "binary",
            ),
            ("puts <<'A\x0cB'; y\x0c+1\nA B\nA\x0cB\n", "y\x0c+1", "call"),
        ] {
            assert_eq!(kind_of(text, operation), kind, "{text:?}");
            let tree = parse(text).unwrap();
            assert!(!tree.root_node().has_error(), "{text:?}");
            let mut stack = vec![tree.root_node()];
            let mut body = None;
            while let Some(node) = stack.pop() {
                if node.kind() == "heredoc_body" {
                    body = Some(node);
                }
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
            }
            // `A B` の行で閉じず、改ページを含む終端語まで続く。
            let body = &text[body.expect("heredoc の本文").byte_range()];
            assert!(body.contains("\nA B\nA\x0cB"), "{text:?} {body:?}");
        }
    }

    /// 本文の改ページは手がかりではない。Ruby と同じく空白として読む。
    #[test]
    fn a_form_feed_in_the_source_reads_as_whitespace() {
        let tree = parse("puts\x0c+1\n").unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{}", root.to_sexp());
        assert_eq!(
            root.named_child(0).unwrap().kind(),
            "call",
            "{}",
            root.to_sexp()
        );
    }

    /// 仮想の改ページを削除した木の全ノード (匿名・幅 0 のノードも含む) が、原文のバイト範囲と
    /// UTF-8 バイトで数える TSPoint を返すことを確かめる。
    fn assert_original_coordinates(tree: &tree_sitter::Tree, text: &str) {
        let mut points = Vec::with_capacity(text.len() + 1);
        let mut point = tree_sitter::Point::new(0, 0);
        points.push(point);
        for byte in text.bytes() {
            if byte == b'\n' {
                point.row += 1;
                point.column = 0;
            } else {
                point.column += 1;
            }
            points.push(point);
        }
        assert_eq!(tree.root_node().end_byte(), text.len(), "{text:?}");
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            let range = node.byte_range();
            assert!(
                range.start <= range.end && range.end <= text.len(),
                "{node:?} {text:?}"
            );
            assert!(
                text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
                "{node:?} {text:?}"
            );
            assert_eq!(
                node.start_position(),
                points[range.start],
                "{node:?} {text:?}"
            );
            assert_eq!(node.end_position(), points[range.end], "{node:?} {text:?}");
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
    }

    /// 空白を置けない行継続でも、字句上で束縛済みの名前だけ二項演算として読む。複数の挿入や
    /// UTF-8・CRLF が混じっても、呼び出し側へ返す座標は原文のまま。
    #[test]
    fn a_virtual_hint_keeps_original_ranges_and_points() {
        for (text, snippet, kind) in [
            ("x = 1\nx\\\n+1\n", "x\\\n+1", "binary"),
            ("x = 1\r\nx\\\r\n+1\r\n", "x\\\r\n+1", "binary"),
            ("値 = 1\n値\\\n+1\n", "値\\\n+1", "binary"),
            ("値 = 1\r\n値\\\r\n+1\r\n", "値\\\r\n+1", "binary"),
            ("x = 1\nx\\\n\\\r\n+1\n", "x\\\n\\\r\n+1", "binary"),
            ("x = 1\nx\\\n+1\nx\\\n+2\n", "x\\\n+2", "binary"),
            ("値 = 1\r\n値\\\r\n+1\r\n値\\\n+2\n", "値\\\n+2", "binary"),
            ("x = 1\nx %(x\\\n+1)\n", "x\\\n+1", "binary"),
            ("[1].map { |x| x\\\n+1 }\n", "x\\\n+1", "binary"),
            (
                "x = []\nx\\\n[0].each { |y| y }\n",
                "x\\\n[0]",
                "element_reference",
            ),
            ("x = 1\nx\\\n**opts\n", "x\\\n**opts", "binary"),
            ("x = 1\nx\\\n%(1)\n", "x\\\n%(1)", "binary"),
            ("s = \"a\x0cb\"\nx = 1\nx\\\n+1\n", "x\\\n+1", "binary"),
            (
                "x = 1\nx\\\n+1\nputs <<'A\x0cB'\nA B\nA\x0cB\nx\\\n+2\n",
                "x\\\n+2",
                "binary",
            ),
            ("x = 1\nx\\\n+1\nputs\x0c+2\n", "puts\x0c+2", "call"),
            ("unknown\\\n+1\n", "unknown\\\n+1", "call"),
            ("unknown\\\n+1, 2\n", "unknown\\\n+1, 2", "call"),
            ("x\\\n+1\nx = 1\n", "x\\\n+1", "call"),
            ("x = 1\nobj.x\\\n+1\n", "obj.x\\\n+1", "call"),
            ("x = 1\ndef m\n  x\\\n+1\nend\n", "x\\\n+1", "call"),
        ] {
            let tree = parse(text).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{text:?}: {}",
                tree.root_node().to_sexp()
            );
            // ブロック本文は式と同じ範囲を持つことがあるので、外側の block_body ではなく
            // その範囲の中に目的の式があることを確かめる。
            let start = text.find(snippet).unwrap();
            let range = start..start + snippet.len();
            let mut stack = vec![tree.root_node()];
            let mut found = false;
            while let Some(node) = stack.pop() {
                if node.kind() == kind && node.byte_range() == range {
                    found = true;
                    break;
                }
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
            }
            assert!(
                found,
                "{text:?}: {kind} {range:?}: {}",
                tree.root_node().to_sexp()
            );
            assert_original_coordinates(&tree, text);
        }
    }

    /// 二項演算へ読み直した結果の構文エラーを、引数呼び出しへ戻して隠さない。
    #[test]
    fn a_virtual_hint_keeps_syntax_errors() {
        for text in [
            "x = 1\nx\\\n+1, 2\n",
            "x = 1\r\nx\\\r\n+1, 2\r\n",
            "x = 1\nx\\\n+1 do\nend\n",
            "[1].each { |x| x\\\n+1, 2 }\n",
        ] {
            let tree = parse(text).unwrap();
            assert!(
                tree.root_node().has_error(),
                "{text:?}: {}",
                tree.root_node().to_sexp()
            );
            assert_original_coordinates(&tree, text);
        }
    }

    /// 元の改ページと改行は行継続ではない。仮想の改ページを消す処理が、実際の改ページを消したり
    /// 行をつないだりしない。
    #[test]
    fn original_form_feeds_and_real_line_breaks_remain_distinct() {
        for text in ["x = 1\nx\x0c\n+1\n", "x = 1\r\nx\x0c\r\n+1\r\n"] {
            let tree = parse(text).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{text:?}");
            assert_eq!(root.named_child_count(), 3, "{text:?}: {}", root.to_sexp());
            assert_eq!(root.named_child(1).unwrap().kind(), "identifier");
            assert_eq!(root.named_child(2).unwrap().kind(), "unary");
            assert_original_coordinates(&tree, text);
        }
    }

    /// heredoc をシフトへ読み直すと補間内の代入が消える。先に仮想の改ページで読ませた後でも、
    /// 根拠の無くなった挿入は残さない。
    #[test]
    fn an_inserted_hint_whose_binding_disappears_is_withdrawn() {
        let text = "x = []\nx <<EOS\n#{y = 1}\nEOS\ny\\\n+1\n";
        assert_eq!(kind_of(text, "x <<EOS"), "binary");
        assert_eq!(kind_of(text, "y\\\n+1"), "call");
        let tree = parse(text).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );
        assert_original_coordinates(&tree, text);
    }

    /// 挿入の無い通常経路は本文を借り、置き換えだけなら同じ長さのバッファ 1 つで済ませる。
    #[test]
    fn ordinary_parses_do_not_allocate_an_inserted_source() {
        let text = "x = 1\nx +1\n";
        let unchanged = super::State::default().apply(text);
        assert!(matches!(unchanged.analysis, std::borrow::Cow::Borrowed(_)));
        assert!(unchanged.inserted.is_none());
        assert!(unchanged.deletions.is_empty());
        let replaced = super::State {
            blanked: Vec::new(),
            hints: vec![crate::lexical_locals::Hint::Replace(
                text.rfind(' ').unwrap(),
            )],
        }
        .apply(text);
        assert!(replaced.inserted.is_none());
        assert!(replaced.deletions.is_empty());
        assert_eq!(replaced.text().len(), text.len());
    }
}
