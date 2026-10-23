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
//! 経路でも呼び出し側が持つ原文に対応する。`?2` を文字リテラルと誤認する位置には、`?` の
//! 直後へ空白を挿入し、同じ座標の復元を使う。名前が未束縛でも `?foo` のように識別文字が
//! 2 文字続く場合は三項演算子なので、同じ手がかりを置く。先頭 Unicode の複数字は
//! Prism と whitequark で解釈が違うので、設定された engine の規則を使う。
//!
//! 改ページは grammar への手がかりで、VT は演算子前の空白として grammar が読めないので、
//! 本文にもともとある両方の制御文字のうち、字句の空白にあるものは空白に置き換えて読む。
//! 文字列・heredoc の終端語・コメントのようにトークンの中にある制御文字は残す (`<<'A\fB'` の終端語を変えると本文の
//! 終わりが変わる)。character と percent literal の区切りは grammar が制御文字をトークン外へ
//! 落とすこともあるので、原本の Prism token で補う。現在の木で同じ開始が演算子で、その直前の
//! 名前が字句束縛済みなら、終了側も一緒にこの保護を撤回する。回復した binary だけを根拠にしない。
//!
//! どちらも読み方しだいで変わる。`x <<EOS` の `<<` を手がかりでシフトと読むと、heredoc だった
//! 本文はコードやコメントになり、そこにあった代入も改ページも意味が変わる。そこで「手がかりの
//! 位置の集合」と「空白にする元の改ページの集合」を状態にして、読むたびに両方をいまの木から
//! 計算し直し、変わらなくなるまで繰り返す。

use std::borrow::Cow;
use std::cell::RefCell;

use tree_sitter::{InputEdit, Node, Parser, Point, Tree};

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

/// 検査と補正後の再検査では、選択した engine と版の正規表現の束縛・文字リテラルの規則を使う。
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
    let controls: Vec<usize> = text
        .bytes()
        .enumerate()
        .filter_map(|(position, byte)| matches!(byte, b'\x0b' | b'\x0c').then_some(position))
        .collect();
    // grammar が character や percent literal の制御文字を葉外へ落とす場合だけ補う。
    // 通常の本文には Prism の追加解析を行わない。
    let literals = if controls.is_empty() {
        Vec::new()
    } else {
        sonicop_prism_compat::control_literals(text.as_bytes())
    };
    let mut probes: Vec<_> = literals.iter().map(|literal| literal.start).collect();
    probes.sort_unstable();
    probes.dedup();
    // percent literal の中の式のように、読み直して初めて呼び出しに見える位置もあるので、
    // 状態が変わらなくなるまで回す。
    settle(State::default(), |state| {
        let readable = state.apply(text);
        // 読み直した木に構文エラーがあっても、束縛の判定に従った読み方を使う。
        // `x = 1; x +1, 2` は Ruby でも構文エラーで、手がかりのない木はそれを隠す。
        let mut tree = parse_raw(readable.text())?;
        readable.restore_coordinates(&mut tree);
        let (hints, bound_positions) = if probes.is_empty() {
            (
                crate::lexical_locals::binary_operator_hints(
                    tree.root_node(),
                    &readable.analysis,
                    &state.hints,
                    semantics,
                ),
                Vec::new(),
            )
        } else {
            let analysis = crate::lexical_locals::operator_analysis(
                tree.root_node(),
                &readable.analysis,
                &state.hints,
                semantics,
                &probes,
                &literals,
            );
            (analysis.hints, analysis.bound_positions)
        };
        let next = State {
            blanked: whitespace_controls(&tree, &controls, &literals, &bound_positions),
            hints,
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
    /// 空白に置き換える、元の本文の VT と改ページの位置。
    blanked: Vec<usize>,
    /// 二項・三項演算を伝える手がかり。置き換えは `blanked` と重なれば手がかりが勝つ。
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
            .filter(|hint| !matches!(hint, Hint::Replace(_)))
            .count();
        let mut deletions = Vec::with_capacity(count);
        let inserted = (count != 0).then(|| {
            let mut readable = String::with_capacity(analysis.len() + count);
            let mut from = 0;
            let mut point = Point::new(0, 0);
            for &hint in &self.hints {
                let (position, marker) = match hint {
                    Hint::Replace(_) => continue,
                    Hint::Insert(position) => (position, '\x0c'),
                    Hint::Ternary(position) => (position, ' '),
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
                readable.push(marker);
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
        // 仮想の改ページも空白も、末尾から消せば前の位置や列は変わらない。編集後に取り出すノードは
        // すべて原文の座標を返す。以後の解析に、挿入した改ページの座標を持ち込まない。
        for deletion in self.deletions.iter().rev() {
            tree.edit(deletion);
        }
    }
}

/// 元の VT・改ページのうち、いまのトークンと literal 区切りの外にあるもの。
fn whitespace_controls(
    tree: &Tree,
    controls: &[usize],
    literals: &[sonicop_prism_compat::ControlLiteral],
    bound_positions: &[usize],
) -> Vec<usize> {
    if controls.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut operators = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        if node.kind() == "binary" {
            if let Some(operator) = node.child_by_field_name("operator")
                && operator.kind() == "%"
            {
                operators.push(operator.start_byte());
            }
        } else if node.kind() == "conditional" {
            let mut children = node.walk();
            operators.extend(
                node.children(&mut children)
                    .filter(|child| child.kind() == "?")
                    .map(|child| child.start_byte()),
            );
        }
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
    operators.sort_unstable();
    // grammar は未知名の literal も binary+ERROR へ回復するため、形だけで保護を撤回しない。
    // 同じ字句位置で束縛が成立した場合だけ終了側も外し、次の State.blanked へ反映する。
    let mut protected: Vec<_> = literals
        .iter()
        .filter(|literal| protected_literal(literal.start, &operators, bound_positions))
        .flat_map(|literal| literal.positions.iter().copied())
        .collect();
    protected.sort_unstable();
    controls
        .iter()
        .copied()
        .filter(|&position| {
            if protected.binary_search(&position).is_ok() {
                return false;
            }
            let next = tokens.partition_point(|range| range.end <= position);
            !tokens
                .get(next)
                .is_some_and(|range| range.start <= position)
        })
        .collect()
}

fn protected_literal(start: usize, operators: &[usize], bound_positions: &[usize]) -> bool {
    operators.binary_search(&start).is_err() || bound_positions.binary_search(&start).is_err()
}

/// 同じ来歴付き束縛で、literal 開始を偽の演算子として補正することだけを防ぐ。
/// 本文・補間コードの operator を範囲全体で除外せず、未対応の AST 自体も書き換えない。
pub(crate) fn protected_control_starts(
    root: Node<'_>,
    text: &str,
    semantics: crate::regexp_ast::Semantics,
    operators: &[usize],
) -> Vec<usize> {
    if !text.bytes().any(|byte| matches!(byte, b'\x0b' | b'\x0c')) {
        return Vec::new();
    }
    let literals = sonicop_prism_compat::control_literals(text.as_bytes());
    if literals.is_empty() {
        return Vec::new();
    }
    let mut probes: Vec<_> = literals.iter().map(|literal| literal.start).collect();
    probes.sort_unstable();
    probes.dedup();
    let analysis =
        crate::lexical_locals::operator_analysis(root, text, &[], semantics, &probes, &literals);
    probes.retain(|&start| protected_literal(start, operators, &analysis.bound_positions));
    probes
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

    /// 三項演算子と文字リテラルの区別は、名前のその位置での束縛による。回復で `?f` が
    /// arguments の外へ出る形や、入れ子で初めて見える三項演算子も読み直す。
    #[test]
    fn bound_questions_are_ternaries_and_unbound_questions_are_characters() {
        for (source, conditionals, characters) in [
            ("x = 1\nputs(x ?2:3)\n", 1, 0),
            ("x = 1\nputs(x\x0b?2:3)\n", 1, 0),
            ("x = 1\nputs(x\r?2:3)\n", 1, 0),
            ("値 = 1\nputs(値 ?2:3)\n", 1, 0),
            ("def m(🤔, 🏷)\n  puts(🤔 ?\"\":\"🚫 \")\nend\n", 1, 0),
            ("x = 1\nputs(x ?foo : bar)\n", 1, 0),
            ("x = 1\nputs(x ?x ?2:3 : 4)\n", 2, 0),
            ("x = 1\nputs(x ?2:x ?3:4)\n", 2, 0),
            ("x = 1\nputs(x ? ?a : ?b)\n", 1, 2),
            ("x = 1\n[1].each { puts(x ?2:3) }\n", 1, 0),
            ("def m(x)\n  puts(x ?2:3)\nend\n", 1, 0),
            ("[1].map { |x| x ?2:3 }\n", 1, 0),
            ("[1].map { _1 ?2:3 }\n", 1, 0),
            ("x = 1\nputs(x\\\n?2:3)\n", 1, 0),
            ("値 = 1\nputs(値 ?2 : 値\\\n+1 + 値 +1)\n", 1, 0),
            ("x = 1\nputs(x\x0c?2:3)\n", 1, 0),
            ("puts(missing ?foo : bar)\n", 1, 0),
            ("puts(missing ?true : false)\n", 1, 0),
            ("puts(missing ?12 : 3)\n", 1, 0),
            ("puts(missing ?aあ : bar)\n", 1, 0),
            ("puts(missing ?あい : bar)\n", 1, 0),
            ("puts(obj.missing ?foo : bar)\n", 1, 0),
            ("def m; puts(yield ?foo : bar); end\n", 1, 0),
            ("def m; puts(super ?foo : bar); end\n", 1, 0),
            ("puts(defined? missing ?foo : bar)\n", 1, 0),
            ("puts(defined? missing ?a)\n", 0, 1),
            ("def m; puts(yield ?あ); end\n", 0, 1),
            ("def m; puts(super ?\\n); end\n", 0, 1),
            ("puts(missing ?2)\n", 0, 1),
            ("puts(missing ?a)\n", 0, 1),
            ("puts(missing ?あ)\n", 0, 1),
            ("puts(missing ?\\n)\n", 0, 1),
            ("puts(obj.missing ?a)\n", 0, 1),
            ("puts(x ?2)\nx = 1\n", 0, 1),
            ("x = 1\nputs(obj.x ?2)\n", 0, 1),
            ("x = 1\ndef m\n  puts(x ?2)\nend\n", 0, 1),
            ("x = 1\nclass Foo\n  puts(x ?2)\nend\n", 0, 1),
            ("[1].each { x = 1 }\nputs(x ?2)\n", 0, 1),
            ("puts(missing\x0c?2)\n", 0, 1),
            ("x = 1\nputs(x?2)\n", 0, 0),
        ] {
            for text in [source.to_owned(), source.replace('\n', "\r\n")] {
                let tree = parse(&text).unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{text:?}: {}",
                    tree.root_node().to_sexp()
                );
                let found = kinds(&tree);
                assert_eq!(
                    found.iter().filter(|kind| *kind == "conditional").count(),
                    conditionals,
                    "{text:?}: {found:?}"
                );
                assert_eq!(
                    found.iter().filter(|kind| *kind == "character").count(),
                    characters,
                    "{text:?}: {found:?}"
                );
                assert_original_coordinates(&tree, &text);
            }
        }
    }

    /// `x ?2` のような colon の欠落を、有効な文字リテラルの引数へ戻して隠さない。
    #[test]
    fn a_ternary_hint_keeps_missing_colon_errors() {
        for source in [
            "x = 1\nputs(x ?2)\n",
            "x = 1\nputs(x ?2,3)\n",
            "puts(missing ?foo)\n",
        ] {
            for text in [source.to_owned(), source.replace('\n', "\r\n")] {
                let tree = parse(&text).unwrap();
                assert!(
                    tree.root_node().has_error(),
                    "{text:?}: {}",
                    tree.root_node().to_sexp()
                );
                assert_original_coordinates(&tree, &text);
            }
        }
    }

    /// シフトへ読み直した後で補間の代入が消えれば、同じ原文の位置にあった三項の手がかりも撤回する。
    /// 元の改ページを空白にする判定も、挿入を消した座標の木を使う。
    #[test]
    fn a_ternary_hint_is_withdrawn_with_its_binding() {
        for source in [
            "x = []\nx <<EOS\n#{y = 1}\nEOS\nputs(y ?2)\n",
            "x = []\nx <<EOS\n#{y = 1}\nEOS\nputs(y\x0c?2)\n",
        ] {
            for text in [source.to_owned(), source.replace('\n', "\r\n")] {
                let tree = parse(&text).unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{text:?}: {}",
                    tree.root_node().to_sexp()
                );
                let found = kinds(&tree);
                assert!(
                    !found.iter().any(|kind| kind == "conditional"),
                    "{text:?}: {found:?}"
                );
                assert_eq!(
                    found.iter().filter(|kind| *kind == "character").count(),
                    1,
                    "{text:?}: {found:?}"
                );
                assert_original_coordinates(&tree, &text);
            }
        }
    }

    /// 旧版で有効な文字引数＋and/修飾子を、現 Ruby の Unicode 規則へ変えて壊さない。
    #[test]
    fn unicode_question_literals_follow_the_selected_engine() {
        for (target, engine, modern) in [
            ("2.7", "default", false),
            ("3.3", "default", true),
            ("3.4", "default", true),
            ("3.3", "parser_whitequark", false),
            ("3.4", "parser_whitequark", false),
            ("3.3", "parser_prism", true),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(".rubocop.yml");
            std::fs::write(
                &path,
                format!("AllCops:\n  TargetRubyVersion: {target}\n  ParserEngine: {engine}\n"),
            )
            .unwrap();
            let config = crate::config::Config::load(Some(&path), directory.path()).unwrap();
            let semantics = crate::regexp_ast::Semantics::from_config(&config);
            for ending in ["\n", "\r\n"] {
                for tail in [
                    "and false",
                    "or false",
                    "rescue nil",
                    "if true",
                    "unless false",
                    "while false",
                    "until true",
                ] {
                    let source = format!("missing ?あ{tail}{ending}");
                    let tree = super::parse_configured(&source, semantics).unwrap();
                    assert_eq!(
                        tree.root_node().has_error(),
                        modern,
                        "{target}/{engine} {source:?}: {}",
                        tree.root_node().to_sexp()
                    );
                    let found = kinds(&tree);
                    assert_eq!(
                        found.iter().filter(|kind| *kind == "character").count(),
                        usize::from(!modern),
                        "{target}/{engine} {source:?}: {found:?}"
                    );
                    assert_original_coordinates(&tree, &source);
                    // 設定のない入口は従来どおり現 Ruby の物理的な読み方を保つ。
                    assert!(
                        parse(&source).unwrap().root_node().has_error(),
                        "{source:?}"
                    );
                }
                for source in [
                    "puts(missing ?foo : bar)\n",
                    "puts(missing ?あ)\n",
                    "puts(missing ?\\n)\n",
                ] {
                    let source = source.replace('\n', ending);
                    let tree = super::parse_configured(&source, semantics).unwrap();
                    assert!(
                        !tree.root_node().has_error(),
                        "{target}/{engine} {source:?}: {}",
                        tree.root_node().to_sexp()
                    );
                    assert_original_coordinates(&tree, &source);
                }
            }
        }
    }

    /// Ruby の水平空白 VT は束縛を変えず、literal とコメント内の VT も同じ本文に残す。
    #[test]
    fn vertical_tabs_are_whitespace_between_tokens_only() {
        for (source, snippet, kind) in [
            ("unknown\x0b+1\n", "unknown\x0b+1", "call"),
            ("unknown\x0b*args\n", "unknown\x0b*args", "call"),
            ("unknown\x0b**kwargs\n", "unknown\x0b**kwargs", "call"),
            ("unknown\x0b&blk\n", "unknown\x0b&blk", "call"),
            ("unknown\x0b/re/\n", "unknown\x0b/re/", "call"),
            ("%w[a\x0bb]\n", "%w[a\x0bb]", "string_array"),
            ("x = 1\nx\x0b+1\n", "x\x0b+1", "binary"),
            ("x = 1\nx\x0b**opts\n", "x\x0b**opts", "binary"),
            ("unknown\x0b+1, 2\n", "unknown\x0b+1, 2", "call"),
            ("x = 1\nx\x0b\\\n+1\n", "x\x0b\\\n+1", "binary"),
            ("foo.zero?\x0b{}\n", "foo.zero?\x0b{}", "call"),
            ("x = 1\nputs(x\x0b?2:3)\n", "x\x0b?2:3", "conditional"),
            ("s = \"a\x0bb\"\nunknown\x0b+1\n", "unknown\x0b+1", "call"),
            ("r = /a\x0bb/\nunknown\x0b+1\n", "unknown\x0b+1", "call"),
            (
                "# comment\x0bkept\nunknown\x0b+1\n",
                "unknown\x0b+1",
                "call",
            ),
            (
                "puts <<'A\x0bB'\nA B\nA\x0bB\nunknown\x0b+1\n",
                "unknown\x0b+1",
                "call",
            ),
            (
                "値 = 1\n文字 = \"a\x0bb\"; 正規 = /a\x0cb/ # コメント\x0b保持\n値\x0b+1\n値\\\n+2\nputs(値\x0b?3:4)\n",
                "値\x0b+1",
                "binary",
            ),
        ] {
            for text in [source.to_owned(), source.replace('\n', "\r\n")] {
                let tree = parse(&text).unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{text:?}: {}",
                    tree.root_node().to_sexp()
                );
                assert_original_coordinates(&tree, &text);
                let original_snippet =
                    snippet.replace('\n', if text.contains("\r\n") { "\r\n" } else { "\n" });
                assert_eq!(kind_of(&text, &original_snippet), kind, "{text:?}");
                if source.starts_with("%w") {
                    assert_eq!(
                        kinds(&tree)
                            .iter()
                            .filter(|kind| *kind == "bare_string")
                            .count(),
                        2,
                        "{text:?}"
                    );
                }
                if source.starts_with("foo.zero?") {
                    assert!(
                        tree.root_node()
                            .named_child(0)
                            .unwrap()
                            .child_by_field_name("block")
                            .is_some(),
                        "{text:?}"
                    );
                }
            }
        }
    }

    /// 水平空白 VT が後ろの通常改行を消したり、無効な引数を有効な call に戻したりしない。
    #[test]
    fn vertical_tabs_keep_line_breaks_and_invalid_bound_arguments() {
        for ending in ["\n", "\r\n"] {
            let source = format!("x = 1{ending}x\x0b{ending}+1{ending}");
            let tree = parse(&source).unwrap();
            assert!(!tree.root_node().has_error(), "{source:?}");
            assert_eq!(
                tree.root_node().named_child(1).unwrap().kind(),
                "identifier"
            );
            assert_original_coordinates(&tree, &source);
            for invalid in ["x = 1\nx\x0b+1, 2\n", "x = 1\nx\x0b+1 do\nend\n"] {
                let source = invalid.replace('\n', ending);
                let tree = parse(&source).unwrap();
                assert!(
                    tree.root_node().has_error(),
                    "{source:?}: {}",
                    tree.root_node().to_sexp()
                );
                assert_original_coordinates(&tree, &source);
            }
        }
    }

    /// heredoc がシフトと読まれれば、本文だった VT の字句分類とその中の束縛も計算し直す。
    #[test]
    fn vertical_tabs_are_reclassified_after_literal_recovery() {
        for source in [
            "x = []\nx <<EOS\ny\x0b+1\nEOS\n",
            "x = []\nx <<EOS\n#{y = 1}\nEOS\ny\x0b+1\n",
        ] {
            for text in [source.to_owned(), source.replace('\n', "\r\n")] {
                let tree = parse(&text).unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{text:?}: {}",
                    tree.root_node().to_sexp()
                );
                assert_eq!(kind_of(&text, "y\x0b+1"), "call", "{text:?}");
                assert_original_coordinates(&tree, &text);
            }
        }
    }

    /// grammar が葉外へ落とす既存の literal 制約では、元の VT/FF を空白と誤分類しない。
    /// この保護自体を、grammar が character 全範囲や control 区切りを読める証拠にはしない。
    #[test]
    fn control_literal_boundaries_are_preserved_despite_grammar_recovery() {
        for control in ['\x0b', '\x0c'] {
            for source in [
                format!("puts ?\\{control}\n"),
                format!("%Q{control}a b{control}\n"),
                format!("%Q{control}#{{ {{ \"a\": :b }}; obj.`(1) }}{control}\n"),
            ] {
                let tree = super::parse_raw(&source).unwrap();
                let controls: Vec<_> = source
                    .bytes()
                    .enumerate()
                    .filter_map(|(i, b)| matches!(b, b'\x0b' | b'\x0c').then_some(i))
                    .collect();
                let literals = sonicop_prism_compat::control_literals(source.as_bytes());
                assert!(
                    super::whitespace_controls(&tree, &controls, &literals, &[]).is_empty(),
                    "{source:?}: {literals:?}"
                );
                assert_original_coordinates(&parse(&source).unwrap(), &source);
            }
        }
    }

    /// 実 Ruby と本家の対象版で異なる capture 束縛も、同じ Walk の位置情報で保護を選ぶ。
    #[test]
    fn control_literal_protection_follows_configured_bindings() {
        for (target, engine, bound) in [
            ("2.7", "default", true),
            ("3.3", "parser_whitequark", false),
            ("3.3", "parser_prism", false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(".rubocop.yml");
            std::fs::write(
                &path,
                format!("AllCops:\n  TargetRubyVersion: {target}\n  ParserEngine: {engine}\n"),
            )
            .unwrap();
            let config = crate::config::Config::load(Some(&path), directory.path()).unwrap();
            let semantics = crate::regexp_ast::Semantics::from_config(&config);
            for control in ['\x0b', '\x0c'] {
                for ending in ["\n", "\r\n"] {
                    let text =
                        format!("/#{{'a'}}(?<y>.)/ =~ text; y %Q{control}+ 1{control}{ending}");
                    let tree = super::parse_configured(&text, semantics).unwrap();
                    if bound {
                        assert!(
                            !tree.root_node().has_error(),
                            "{text:?}: {}",
                            tree.root_node().to_sexp()
                        );
                    } else {
                        // unsupported literal の元木を保ち、新しい valid binary へ偽装修復しない。
                        assert_eq!(
                            tree.root_node().to_sexp(),
                            super::parse_raw(&text).unwrap().root_node().to_sexp(),
                            "{target}/{engine} {text:?}"
                        );
                    }
                    assert_original_coordinates(&tree, &text);
                }
            }
        }
    }

    /// literal 本文で回復した偽の宣言が、後続の未知名の literal 保護を撤回しない。
    #[test]
    fn ghost_literal_bindings_do_not_add_an_operator() {
        for source in [
            "%Q\x0by = 1\x0b\ny %Q\x0b+ 1\x0b\n",
            "%Q\x0bdef m(y); end\x0b\ny %Q\x0b+ 1\x0b\n",
        ] {
            for text in [source.to_owned(), source.replace('\n', "\r\n")] {
                let tree = parse(&text).unwrap();
                assert_eq!(
                    tree.root_node().to_sexp(),
                    super::parse_raw(&text).unwrap().root_node().to_sexp(),
                    "{text:?}"
                );
                assert_original_coordinates(&tree, &text);
            }
        }
    }
}
