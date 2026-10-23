//! heredoc の開始 (`<<~X`) と本文の対応。
//!
//! tree-sitter は heredoc を 2 つに分け、`heredoc_beginning` を書いた位置に、`heredoc_body` を
//! その行の後ろに置く。本家の構文木は 1 つのノードなので、利用側で組にし直す。
//!
//! Ruby は本文を、開始を書いた行の次の行から読む。同じ行で開いた heredoc は開いた順に続けて読み、
//! 本文の補間で開いた heredoc は、その補間の行の次の行から、外側の本文の続きより先に読む。
//! そのため入れ子があると、開始の順と本文の順は一致しない。`puts(<<~A, <<~B)` の `A` の本文で
//! `<<~C` を開くと、開始は A・B・C の順、本文は A・C・B の順に並ぶ。位置の順に組にすると B と C を
//! 取り違える。

use tree_sitter::Node;

/// 開始と本文の組。開始の位置の順に並ぶ。対応する本文の無い開始は入らない。
pub(crate) fn pair<'tree>(
    beginnings: impl IntoIterator<Item = Node<'tree>>,
    bodies: impl IntoIterator<Item = Node<'tree>>,
    text: &str,
) -> Vec<(Node<'tree>, Node<'tree>)> {
    let mut beginnings: Vec<Node<'tree>> = beginnings.into_iter().collect();
    let mut bodies: Vec<Node<'tree>> = bodies.into_iter().collect();
    beginnings.sort_by_key(Node::start_byte);
    bodies.sort_by_key(Node::start_byte);
    let mut taken = vec![false; bodies.len()];
    // 行の終わりの改行の位置から、その行で開いた heredoc のうち最後に組にした本文の終わり。
    let mut lines: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut pairs = Vec::with_capacity(beginnings.len());
    for beginning in beginnings {
        let end = beginning.end_byte();
        let line = text
            .get(end..)
            .and_then(|rest| rest.find('\n'))
            .map_or(text.len(), |offset| end + offset);
        // 同じ行で先に開いた heredoc があれば、その本文が終わった後から始まる本文を探す。
        let from = lines.get(&line).copied().unwrap_or(end).max(end);
        let first = bodies.partition_point(|body| body.start_byte() < from);
        let Some(index) = (first..bodies.len()).find(|&index| !taken[index]) else {
            continue;
        };
        taken[index] = true;
        lines.insert(line, bodies[index].end_byte());
        pairs.push((beginning, bodies[index]));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::pair;

    /// heredoc の開始と本文を集めて組にする。どの本文も 2 度は使われないことも確かめる。
    fn paired(tree: &tree_sitter::Tree, source: &str) -> Vec<(usize, usize, Option<usize>)> {
        let mut beginnings = Vec::new();
        let mut bodies = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "heredoc_beginning" => beginnings.push(node),
                "heredoc_body" => bodies.push(node),
                _ => {}
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        let pairs = pair(beginnings, bodies, source);
        let used: HashSet<usize> = pairs.iter().map(|(_, body)| body.id()).collect();
        assert_eq!(used.len(), pairs.len(), "本文を 2 度使った: {source:?}");
        pairs
            .into_iter()
            .map(|(beginning, body)| {
                let mut cursor = body.walk();
                let end = body
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "heredoc_end")
                    .map(|end| end.start_position().row);
                (beginning.start_byte(), body.start_byte(), end)
            })
            .collect()
    }

    fn parse(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    /// 組ごとに、開始の行と終端語の行 (0 始まり)。開始が `<<` の位置にあり、本文が開始より後の
    /// 行から始まることも確かめる。
    fn pairs(source: &str) -> Vec<(usize, usize)> {
        let tree = parse(source);
        paired(&tree, source)
            .into_iter()
            .map(|(beginning, body, end)| {
                let row = row_of(source, beginning);
                assert!(source[beginning..].starts_with("<<"), "{source:?}");
                assert!(body_row(source, body) > row, "{source:?}");
                (row, end.expect("終端のある本文"))
            })
            .collect()
    }

    fn row_of(source: &str, position: usize) -> usize {
        source[..position].matches('\n').count()
    }

    /// 本文の 1 行目。tree-sitter の本文は、開始を書いた行の終わりの改行 (CRLF なら `\r`) から
    /// 始まる。
    fn body_row(source: &str, position: usize) -> usize {
        let rest = &source[position..];
        assert!(
            rest.starts_with('\n') || rest.starts_with("\r\n"),
            "本文が改行から始まらない: {source:?} {position}"
        );
        row_of(source, position) + 1
    }

    /// 同じ行の heredoc は開いた順に、本文の補間で開いた heredoc はその行の次から読む。
    /// 開始は A・B・C の順、本文は A・C・B の順に並ぶ。
    #[test]
    fn bodies_pair_in_the_order_ruby_reads_them() {
        let source = "puts(<<~A, <<~B)\n  #{puts(<<~C)}\n    #{x}\n  C\nA\n  #{y}\nB\n";
        assert_eq!(pairs(source), [(0, 4), (0, 6), (1, 3)]);
        let crlf = source.replace('\n', "\r\n");
        assert_eq!(pairs(&crlf), [(0, 4), (0, 6), (1, 3)]);
    }

    /// 同じ終端語を使い回しても、外側と内側を読む順で組にする。
    #[test]
    fn a_reused_terminator_pairs_by_reading_order() {
        let source = "puts(<<~E, <<~F)\n  #{puts(<<~E)}\n    inner\n  E\nE\n  second\nF\n";
        assert_eq!(pairs(source), [(0, 4), (0, 6), (1, 3)]);
    }

    /// 1 行に 3 つ以上開いても、前の本文が終わった後から順に読む。
    #[test]
    fn many_heredocs_on_one_line_are_read_one_after_another() {
        let source = "puts(<<~A, <<~B, <<~C, <<~D)\n  a\nA\n  b\nB\n  c\nC\n  d\nD\n";
        assert_eq!(pairs(source), [(0, 2), (0, 4), (0, 6), (0, 8)]);
    }

    /// 1 つの補間で 2 つ開くと、その行の次から 2 つの本文を順に読み、外側の本文はその後に続く。
    #[test]
    fn heredocs_opened_together_in_an_interpolation_follow_that_line() {
        let source = "puts(<<~A)\n  #{foo(<<~B, <<~C)}\n  b\n  B\n  c\n  C\n  a\nA\n";
        assert_eq!(pairs(source), [(0, 7), (1, 3), (1, 5)]);
    }

    /// 入れ子が何段あっても、内側の本文は開いた行の次から読む。
    #[test]
    fn nested_heredocs_pair_at_every_depth() {
        let source =
            "puts(<<~A)\n  #{foo(<<~B)}\n    #{bar(<<~C)}\n      c\n    C\n    b\n  B\n  a\nA\n";
        assert_eq!(pairs(source), [(0, 8), (1, 6), (2, 4)]);
    }

    /// 終端の無い heredoc も、その行の次から始まる本文と組にする。本文は 1 度しか使わない。
    #[test]
    fn an_unterminated_heredoc_still_pairs_with_its_body() {
        let source = "puts(<<~A, <<~B)\n  a\nA\n  b\n";
        let tree = parse(source);
        let pairs = paired(&tree, source);
        assert_eq!(pairs.len(), 2, "{}", tree.root_node().to_sexp());
        assert_eq!(pairs[0].0, source.find("<<~A").unwrap());
        assert_eq!(body_row(source, pairs[0].1), 1);
        assert_eq!(pairs[0].2, Some(2));
        assert_eq!(pairs[1].0, source.find("<<~B").unwrap());
        assert_eq!(body_row(source, pairs[1].1), 3);
    }
}
