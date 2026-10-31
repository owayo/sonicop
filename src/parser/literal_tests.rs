//! 文法側の独立した literal_contents_test.rs の期待を、Sonicop の parse 入口でも保つ。

use tree_sitter::{Node, Point};

use super::parse;

fn point(text: &str, offset: usize) -> Point {
    let before = &text.as_bytes()[..offset];
    Point::new(
        before.iter().filter(|&&byte| byte == b'\n').count(),
        before
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(offset, |last| offset - last - 1),
    )
}

fn coordinates(root: Node<'_>, text: &str) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        assert!(text.is_char_boundary(node.start_byte()));
        assert!(text.is_char_boundary(node.end_byte()));
        assert_eq!(node.start_position(), point(text, node.start_byte()));
        assert_eq!(node.end_position(), point(text, node.end_byte()));
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

fn parts(node: Node<'_>, text: &str, expected: &[(&str, &str)], mut offset: usize) -> usize {
    assert_eq!(node.named_child_count(), expected.len(), "{text:?}");
    for (index, (kind, contents)) in expected.iter().enumerate() {
        let part = node.named_child(index as u32).unwrap();
        assert_eq!(part.kind(), *kind, "{text:?}");
        assert_eq!(
            part.byte_range(),
            offset..offset + contents.len(),
            "{text:?}"
        );
        assert_eq!(&text[part.byte_range()], *contents, "{text:?}");
        offset += contents.len();
    }
    offset
}

#[test]
fn ordinary_literal_parts_stay_direct_children_with_original_coordinates() {
    // 上流v26.10.107のparts期待を使い、単一要素の隠れた部分木の削減で位置を変えない。
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("", &[]),
        ("文字", &[("string_content", "文字")]),
        ("\\n", &[("escape_sequence", "\\n")]),
        ("#{value}", &[("interpolation", "#{value}")]),
        (
            "before\\n#{value}after",
            &[
                ("string_content", "before"),
                ("escape_sequence", "\\n"),
                ("interpolation", "#{value}"),
                ("string_content", "after"),
            ],
        ),
    ];
    for ending in ["\n", "\r\n"] {
        for (prefix, suffix, kind) in [
            ("\"", "\"", "string"),
            ("%Q[", "]", "string"),
            ("/", "/im", "regex"),
            ("%r[", "]im", "regex"),
            (":\"", "\"", "delimited_symbol"),
            ("`", "`", "subshell"),
            ("%x[", "]", "subshell"),
        ] {
            for (body, expected) in cases {
                let leading = format!(" \t{ending}");
                let text = format!("{leading}{prefix}{body}{suffix}{ending}");
                let tree = parse(&text).unwrap();
                let root = tree.root_node();
                assert!(!root.has_error(), "{text:?}: {}", root.to_sexp());
                let literal = root.named_child(0).unwrap();
                assert_eq!(literal.kind(), kind);
                assert_eq!(
                    literal.byte_range(),
                    leading.len()..text.len() - ending.len()
                );
                assert_eq!(
                    parts(literal, &text, expected, leading.len() + prefix.len()),
                    literal.end_byte() - suffix.len()
                );
                coordinates(root, &text);
            }
        }
    }
}

#[test]
fn interpolated_word_arrays_keep_aliased_elements_and_interpolation_boundaries() {
    for ending in ["\n", "\r\n"] {
        // 上流がpartsの期待を定義する補間付き配列。小文字系は実CLI側で別に照合する。
        for (prefix, kind) in [("%W[", "bare_string"), ("%I[", "bare_symbol")] {
            let leading = format!(" \t{ending}");
            let text = format!("{leading}{prefix}one two\\ three #{{value}}]{ending}");
            let tree = parse(&text).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{text:?}: {}", root.to_sexp());
            let array = root.named_child(0).unwrap();
            assert_eq!(array.named_child_count(), 3);
            let mut offset = leading.len() + prefix.len();
            for (index, (word_text, expected)) in [
                ("one", vec![("string_content", "one")]),
                (
                    "two\\ three",
                    vec![
                        ("string_content", "two"),
                        ("escape_sequence", "\\ "),
                        ("string_content", "three"),
                    ],
                ),
                ("#{value}", vec![("interpolation", "#{value}")]),
            ]
            .into_iter()
            .enumerate()
            {
                let word = array.named_child(index as u32).unwrap();
                assert_eq!(word.kind(), kind, "{text:?}");
                assert_eq!(word.byte_range(), offset..offset + word_text.len());
                assert_eq!(parts(word, &text, &expected, offset), word.end_byte());
                offset += word_text.len() + 1;
            }
            coordinates(root, &text);
        }
    }
}
