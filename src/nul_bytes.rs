//! Reading a source the way Ruby's lexer reads one that holds a NUL byte.
//!
//! Ruby reads a NUL three different ways, and only one of them ends the program:
//!
//! | where the NUL is | Ruby reads it as | what this module does |
//! |---|---|---|
//! | in code | the end of the program | truncate there |
//! | in a comment | part of the comment's text | replace it with a space |
//! | **in a literal** | **a character of the string** | **leave it exactly as written** |
//!
//! tree-sitter cannot do the first two: NUL is its own end-of-input sentinel, so its generated
//! lexer stops the token it is reading and carries on with whatever follows, leaving an error where
//! Ruby saw nothing of the sort. Left alone that costs every cop on the file, since a source that
//! does not parse reports only `Lint/Syntax`.
//!
//! **Inside a literal it needs no help.** The grammar reads `"a\0b"` as one `string_content`
//! spanning the byte, so the third row is a matter of *not* treating that NUL as the end of the
//! program -- which is what this module did until 2026-08-17, refusing seven kinds of literal
//! (`""` / `''` / heredoc / `%w` / `%i` / regexp / `:""`) that `ruby -c` and RuboCop both accept.
//!
//! Rewriting the source once, before anything looks at it, is what keeps all the cops agreeing with
//! RuboCop rather than only the one that reports the parse.

use std::ops::Range;

use tree_sitter::{Node, Parser};

/// The source as Ruby's lexer would have read it, or `None` when there is no NUL to account for --
/// which is every ordinary file, so this costs one scan and nothing else.
pub fn as_ruby_reads_it(text: &str) -> Option<String> {
    if !text.as_bytes().contains(&0) {
        return None;
    }
    let mut text = text.to_owned();
    let (mut comments, mut literals) = parse(&text).map(|tree| ranges(tree.root_node()))?;
    // Where to look for the next NUL. A NUL left in place has to be stepped over, or the scan finds
    // the same byte forever.
    let mut from = 0;
    loop {
        let Some(offset) = text.as_bytes()[from..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|found| from + found)
        else {
            return Some(text);
        };
        // A literal that spans the byte is one Ruby keeps reading, so the NUL stays a character of
        // the string. Replacing it would change the value every cop sees; truncating there refuses
        // a file both `ruby -c` and RuboCop accept.
        if literals
            .iter()
            .any(|literal| literal.start <= offset && offset < literal.end)
        {
            from = offset + 1;
            continue;
        }
        // The grammar breaks the comment off at the byte it cannot read, so a comment ending exactly
        // there is one the NUL was written inside. Anywhere else, Ruby stopped reading.
        if !comments.iter().any(|comment| comment.end == offset) {
            text.truncate(offset);
            return Some(text);
        }
        // A comment runs to the end of its line, so every NUL up to there belongs to it too and can
        // be settled in one pass rather than one re-parse per byte.
        let line_end = text[offset..]
            .find('\n')
            .map_or(text.len(), |position| offset + position);
        let mut bytes = std::mem::take(&mut text).into_bytes();
        for byte in &mut bytes[offset..line_end] {
            if *byte == 0 {
                *byte = b' ';
            }
        }
        // A NUL and a space are both one byte, so every offset the caller holds still lands where it
        // did and the text is still the UTF-8 it was.
        text = String::from_utf8(bytes).expect("a one-byte character replaced another");
        from = line_end;
        let Some(tree) = parse(&text) else {
            return Some(text);
        };
        (comments, literals) = ranges(tree.root_node());
    }
}

fn parse(text: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .ok()?;
    parser.parse(text, None)
}

/// The nodes that hold the text of a literal. `%w` and `%i` wrap theirs in a `bare_string` or a
/// `bare_symbol`, and a regexp and a `:""` symbol in their own node, but the text itself is a
/// `string_content` in every one of them. A heredoc keeps its body outside the expression, in a
/// `heredoc_content` of its own.
const LITERAL_CONTENT_KINDS: &[&str] = &["string_content", "heredoc_content"];

/// The comment ranges and the literal-content ranges, in one walk.
fn ranges(root: Node<'_>) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (mut comments, mut literals) = (Vec::new(), Vec::new());
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" {
            comments.push(node.byte_range());
        } else if LITERAL_CONTENT_KINDS.contains(&node.kind()) {
            literals.push(node.byte_range());
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    (comments, literals)
}

#[cfg(test)]
mod tests {
    use super::as_ruby_reads_it;

    #[test]
    fn a_nul_in_code_ends_the_program() {
        let text = "x = 1\n\u{0} this is ) not ( ruby\n";

        assert_eq!(as_ruby_reads_it(text).as_deref(), Some("x = 1\n"));
    }

    #[test]
    fn a_nul_in_a_comment_is_part_of_the_comment() {
        // The rest of the file still has to be read, and every offset after the NUL has to stay
        // where it was, so the byte is replaced rather than removed.
        let text = "# comment \u{0} with nul\nx = 1\n";

        assert_eq!(
            as_ruby_reads_it(text).as_deref(),
            Some("# comment   with nul\nx = 1\n")
        );
    }

    /// **Ruby keeps a NUL written inside a literal as a character of the string**, and the grammar
    /// reads the literal across it, so the source is handed on exactly as written. Truncating there
    /// refused seven kinds of literal that `ruby -c` and RuboCop 1.89.0 both accept.
    #[test]
    fn a_nul_in_a_literal_is_a_character_of_it() {
        for source in [
            "x = \"a\u{0}b\"\ny = 1\n",
            "x = 'a\u{0}b'\ny = 1\n",
            "x = <<~TXT\n  a\u{0}b\nTXT\ny = 1\n",
            "%w[a\u{0}b]\ny = 1\n",
            "%i[a\u{0}b]\ny = 1\n",
            "x = /a\u{0}b/\ny = 1\n",
            "x = :\"a\u{0}b\"\ny = 1\n",
        ] {
            assert_eq!(
                as_ruby_reads_it(source).as_deref(),
                Some(source),
                "the literal has to reach the cops as it was written"
            );
        }
    }

    /// The three readings in one file: the literal keeps its byte, the comment's becomes a space,
    /// and the one in code still ends the program.
    #[test]
    fn the_three_readings_do_not_interfere() {
        let text = "x = \"a\u{0}b\"\n# c \u{0} d\ny = 1\u{0}\nz = 2\n";

        assert_eq!(
            as_ruby_reads_it(text).as_deref(),
            Some("x = \"a\u{0}b\"\n# c   d\ny = 1")
        );
    }

    #[test]
    fn a_source_without_one_is_left_alone() {
        assert_eq!(as_ruby_reads_it("x = 1\n"), None);
    }
}
