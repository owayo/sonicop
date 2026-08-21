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

/// The byte a NUL inside a comment is read as.
///
/// **Keeping the offsets is not enough; the character class has to survive too.** A space was used
/// until 2026-08-17, and it made two cops disagree with RuboCop, because a space is `\s` and
/// `[[:blank:]]` where Ruby's NUL is neither:
///
/// | source | what the space made of it | what RuboCop sees |
/// |---|---|---|
/// | `# TODO\0 fix it` | `# TODO ` -- a bare annotation keyword | `TODO\0`, which is not one |
/// | `# frozen_string_literal: true\0` | matches the magic comment's `\s*$` | no magic comment |
///
/// `Style/CommentAnnotation` gained 2 offenses and `Style/FrozenStringLiteralComment` lost 43.
/// `\x01` is the stand-in instead: one byte, not whitespace to any of the patterns, does not end a
/// comment, and the grammar reads it (measured, both cops agree with upstream again).
const STAND_IN: u8 = 0x01;

/// The source as Ruby's lexer would have read it, or `None` when there is no NUL to account for --
/// which is every ordinary file, so this costs one scan and nothing else.
pub fn as_ruby_reads_it(text: &str) -> Option<String> {
    if !text.as_bytes().contains(&0) {
        return None;
    }
    let mut text = text.to_owned();
    let (mut comment_ends, mut literals) = parse(&text).map(|tree| ranges(tree.root_node()))?;
    // Where to look for the next NUL. A NUL left in place has to be stepped over, or the scan finds
    // the same byte forever. It only ever moves forward, which is also what stops a re-parse -- one
    // that can turn up a comment behind it -- from sending the walk back over a byte it has already
    // read and so leaving it with nothing to make progress on.
    let mut from = 0;
    loop {
        // **A pass settles every comment the parse in hand already knows about, not just the first
        // one.** Re-parsing after each comment cost a whole parse per comment line, and a parse of
        // a source this broken is anything but cheap: a file of 200 such lines took 5 seconds, one
        // of 800 took 82, and one of 3,200 was still going after three minutes. The re-parse itself
        // has to stay -- reading a NUL as part of its comment is often what lets the *next* line
        // parse at all -- but it belongs once per pass, not once per line.
        let mut settled = Vec::new();
        let stopped_at = loop {
            let Some(offset) = text.as_bytes()[from..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|found| from + found)
            else {
                break None;
            };
            // A literal that spans the byte is one Ruby keeps reading, so the NUL stays a character
            // of the string. Replacing it would change the value every cop sees; truncating there
            // refuses a file both `ruby -c` and RuboCop accept.
            let spanning = literals.partition_point(|literal| literal.end <= offset);
            if literals
                .get(spanning)
                .is_some_and(|literal| literal.start <= offset)
            {
                from = offset + 1;
                continue;
            }
            // The grammar breaks the comment off at the byte it cannot read, so a comment ending
            // exactly there is one the NUL was written inside. Anywhere else, Ruby stopped reading
            // -- but only a parse that already accounts for this pass's replacements is allowed to
            // say so, so the byte goes to the decision below rather than ending the walk here.
            if comment_ends.binary_search(&offset).is_err() {
                break Some(offset);
            }
            // A comment runs to the end of its line, so every NUL up to there belongs to it too.
            // The line ends past the NUL either way, so the walk cannot stall on what it settled.
            let line_end = text[offset..]
                .find('\n')
                .map_or(text.len(), |position| offset + position);
            settled.push(offset..line_end);
            from = line_end;
        };
        if !settled.is_empty() {
            let mut bytes = std::mem::take(&mut text).into_bytes();
            for comment in &settled {
                for byte in &mut bytes[comment.start..comment.end] {
                    if *byte == 0 {
                        *byte = STAND_IN;
                    }
                }
            }
            // One byte replaced another, so every offset the caller holds still lands where it did
            // and the text is still the UTF-8 it was.
            text = String::from_utf8(bytes).expect("a one-byte character replaced another");
        }
        // No NUL is left for a re-parse to change the reading of.
        let Some(offset) = stopped_at else {
            return Some(text);
        };
        // **Truncating is the one reading that cannot be taken back**, so it is only ever decided
        // on a parse of exactly the text being truncated. A pass that replaced nothing already had
        // one, and the byte really is where Ruby stopped. A pass that replaced something reads the
        // byte again against the tree those replacements produce, which is how a comment that a NUL
        // further up the file was hiding gets read as a comment instead of ending the program.
        if settled.is_empty() {
            text.truncate(offset);
            return Some(text);
        }
        let Some(tree) = parse(&text) else {
            return Some(text);
        };
        (comment_ends, literals) = ranges(tree.root_node());
        // Each pass that gets here replaced at least one NUL, so the file runs out of them and the
        // loop ends.
    }
}

#[cfg(test)]
thread_local! {
    /// How many parses the pass above has taken. Counted rather than timed because a wall clock
    /// only ever reports how fast the machine running the test is, where the count is the thing
    /// that used to grow one-for-one with the number of comment lines. One cell per thread, so the
    /// test that reads it needs no lock: `libtest` gives every test its own thread.
    static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn parse(text: &str) -> Option<tree_sitter::Tree> {
    #[cfg(test)]
    PARSES.with(|parses| parses.set(parses.get() + 1));
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

/// Where the comments end and where the literals run, in one walk.
///
/// Both come back ordered so the pass above can search them rather than scan them. A file whose
/// every line carries a NUL asks one question per line, and answering each by walking every node in
/// the file is the same quadratic shape the pass exists to get out of -- 25,600 such lines spent as
/// long in the scan as in everything else put together.
fn ranges(root: Node<'_>) -> (Vec<usize>, Vec<Range<usize>>) {
    let (mut comment_ends, mut literals) = (Vec::new(), Vec::new());
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" {
            comment_ends.push(node.byte_range().end);
        } else if LITERAL_CONTENT_KINDS.contains(&node.kind()) {
            literals.push(node.byte_range());
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    comment_ends.sort_unstable();
    // Merged into disjoint runs, which is what lets one lookup answer "is this byte inside a
    // literal": the walk hands them back in whatever order the stack popped them, and a literal can
    // sit inside another -- a string in the interpolation of a heredoc whose body surrounds it.
    literals.sort_unstable_by_key(|literal| literal.start);
    literals.dedup_by(|literal, run| {
        if literal.start > run.end {
            return false;
        }
        run.end = run.end.max(literal.end);
        true
    });
    (comment_ends, literals)
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
            Some("# comment \u{1} with nul\nx = 1\n")
        );
    }

    /// **The stand-in must not be whitespace.** A space is `\s` and `[[:blank:]]`; Ruby's NUL is
    /// neither, and two cops read the difference -- `# TODO\0 fix it` became a bare `TODO`
    /// annotation, and `# frozen_string_literal: true\0` began matching the magic comment's
    /// `\s*$`, which took the comment away from `Style/FrozenStringLiteralComment` (43 offenses)
    /// and handed it to `Layout/TrailingWhitespace`.
    #[test]
    fn the_stand_in_is_not_whitespace() {
        for text in [
            "# TODO\u{0} fix it\nx = 1\n",
            "# frozen_string_literal: true\u{0}\nx = 1\n",
        ] {
            let read = as_ruby_reads_it(text).expect("the source holds a NUL");
            assert!(
                read.contains('\u{1}'),
                "the NUL has to become the stand-in, not whitespace: {read:?}"
            );
            assert_eq!(
                read.matches(' ').count(),
                text.matches(' ').count(),
                "no space may be added: {read:?}"
            );
            assert_eq!(read.len(), text.len(), "one byte replaced another");
        }
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
            Some("x = \"a\u{0}b\"\n# c \u{1} d\ny = 1")
        );
    }

    #[test]
    fn a_source_without_one_is_left_alone() {
        assert_eq!(as_ruby_reads_it("x = 1\n"), None);
    }

    /// **What the reading costs may not grow with the number of comments it settles.** Settling one
    /// per parse made 200 comment lines take 5 seconds, 800 take 82, and 3,200 take longer than
    /// anyone was willing to wait -- four times the input for sixteen times the time, on a file of
    /// 5.6 KB. The parse that finds these comments is the parse that settles all of them, so the
    /// count stays flat however many there are.
    #[test]
    fn the_parses_do_not_grow_with_the_comments() {
        for lines in [1, 10, 100, 1000] {
            let source = "# c\u{0} x\n".repeat(lines) + "z = 1\n";
            let expected = "# c\u{1} x\n".repeat(lines) + "z = 1\n";

            let (read, parses) = super::PARSES.with(|parses| {
                parses.set(0);
                let read = as_ruby_reads_it(&source);
                (read, parses.get())
            });

            assert_eq!(read.as_deref(), Some(expected.as_str()), "{lines} comments");
            assert!(parses <= 2, "{lines} comments took {parses} parses");
        }
    }

    /// The reading this module took until the pass replaced it: settle the first comment the parse
    /// knows about, re-parse, start again. It is slow, but it is the definition of the answer, so
    /// it is kept here to hold the batched pass to it byte for byte. A deliberate change to how a
    /// NUL is read has to be made here too, or the test below reports it as a disagreement.
    ///
    /// It asks its questions by scanning rather than searching, so the test also holds the ordered
    /// lookups the pass gained to the plain reading of the same ranges.
    fn one_comment_at_a_time(text: &str) -> Option<String> {
        if !text.as_bytes().contains(&0) {
            return None;
        }
        let mut text = text.to_owned();
        let (mut comment_ends, mut literals) =
            super::parse(&text).map(|tree| super::ranges(tree.root_node()))?;
        let mut from = 0;
        loop {
            let Some(offset) = text.as_bytes()[from..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|found| from + found)
            else {
                return Some(text);
            };
            if literals
                .iter()
                .any(|literal| literal.start <= offset && offset < literal.end)
            {
                from = offset + 1;
                continue;
            }
            if !comment_ends.contains(&offset) {
                text.truncate(offset);
                return Some(text);
            }
            let line_end = text[offset..]
                .find('\n')
                .map_or(text.len(), |position| offset + position);
            let mut bytes = std::mem::take(&mut text).into_bytes();
            for byte in &mut bytes[offset..line_end] {
                if *byte == 0 {
                    *byte = super::STAND_IN;
                }
            }
            text = String::from_utf8(bytes).expect("a one-byte character replaced another");
            from = line_end;
            let Some(tree) = super::parse(&text) else {
                return Some(text);
            };
            (comment_ends, literals) = super::ranges(tree.root_node());
        }
    }

    /// The line shapes the two readings could disagree about. A NUL only gets interesting next to
    /// something that decides how the rest of the file lexes -- a `#`, a quote that never closes, a
    /// heredoc marker -- because that is what makes one parse of the file disagree with the next.
    const SHAPES: &[&str] = &[
        "# comment{} tail",
        "#{}",
        "# TODO{} fix",
        "# frozen_string_literal: true{}",
        "x = \"str{}ing\"",
        "y = 'sq{}uote'",
        "z = /re{}gex/",
        "%w[a{}b c]",
        "%i[a{}b]",
        "w = :\"sym{}bol\"",
        "v = \"a{}#{ 1 + 1 }b\"",
        "a = 1{}",
        "def m{}(q); q; end",
        "c = \"quote{} \" + \"more\"",
        "d = 3 # trail{}ing",
        "e = 4",
        "",
        "g = <<~TXT",
        "  here{}doc",
        "TXT",
        "h = \"unclosed{}",
        "i = 5 }{}{",
        "# 日本語{}コメント",
    ];

    /// Where the shapes above are drawn from. A fixed sequence rather than a random one, so that a
    /// disagreement is reproducible from the test alone.
    struct Sequence(u64);

    impl Sequence {
        fn next(&mut self, modulo: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as usize % modulo
        }
    }

    /// **Settling several comments per parse must not change what any of them is read as.** The
    /// risk the pass takes is that it classifies a NUL against a parse made before the comments
    /// ahead of it were settled; the reading it replaced never did. Sources built out of the shapes
    /// above are where the two would come apart if it mattered.
    #[test]
    fn the_batched_pass_reads_what_one_comment_at_a_time_read() {
        let mut sequence = Sequence(20260821);
        for _ in 0..600 {
            let mut source = String::new();
            for _ in 0..=sequence.next(7) {
                let slot = if sequence.next(2) == 0 { "\u{0}" } else { "" };
                source.push_str(&SHAPES[sequence.next(SHAPES.len())].replace("{}", slot));
                source.push_str(if sequence.next(8) == 0 { "\r\n" } else { "\n" });
            }

            assert_eq!(
                as_ruby_reads_it(&source),
                one_comment_at_a_time(&source),
                "the two readings disagree on {source:?}"
            );
        }
    }
}
