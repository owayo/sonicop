//! `ParenthesesCorrector`: taking a pair of parentheses out of the source.

use tree_sitter::Node;

use crate::diagnostic::Edit;
use crate::rules::RuleContext;

/// `ParenthesesCorrector.correct`: the opening parenthesis takes the whitespace after it, and the
/// closing one the whitespace before it.
///
/// `node` is the parenthesized expression, whose first and last characters are the parentheses.
pub(super) fn correct(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Edit> {
    let text = context.source.text();
    let bytes = text.as_bytes();
    let range = node.byte_range();
    // `range_with_surrounding_space(node.loc.begin, side: :right, whitespace: true)`.
    let mut open_end = range.start + 1;
    while bytes.get(open_end).is_some_and(u8::is_ascii_whitespace) {
        open_end += 1;
    }
    let close = range.end - 1;
    // The newline before `)` is kept where a comment sits above it and a chain follows it, which
    // would otherwise pull the chain into the comment.
    let newlines = !comment_above_close_paren(context, node);
    let mut edits = vec![
        Edit {
            start: range.start,
            end: open_end,
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: super::ranges::extended_left(text, close, newlines),
            end: range.end,
            replacement: String::new(),
            safe: true,
        },
    ];
    // `handle_orphaned_comma`: a comma left alone on its own line would not parse, so the
    // whitespace before it goes as well. The heredoc half of upstream's handling is left out --
    // nothing in the corpora reaches it, and it rewrites the comma rather than removing it.
    if let Some(orphaned) = orphaned_comma_start(context, close) {
        edits.push(Edit {
            start: orphaned,
            end: range.end,
            replacement: String::new(),
            safe: true,
        });
    }
    // `ternary_condition?`: `(a) ? b : c` needs the space the parenthesis used to provide.
    if is_ternary_condition_before_question_mark(context, node) {
        edits.push(Edit {
            start: range.end,
            end: range.end,
            replacement: " ".to_owned(),
            safe: true,
        });
    }
    edits
}

/// `only_closing_paren_before_comma?` with `parens_range`: where the run of whitespace that would
/// leave the comma stranded begins.
fn orphaned_comma_start(context: &RuleContext<'_>, close: usize) -> Option<usize> {
    let line = context.source.line(context.source.line_column(close).0);
    let after_indent = line.trim_start();
    if !after_indent.starts_with(')') {
        return None;
    }
    if !after_indent[1..].trim_start().starts_with(',') {
        return None;
    }
    // `range_with_surrounding_space(side: :left, newlines: true, whitespace: true,
    // continuations: true)`.
    let bytes = context.source.text().as_bytes();
    let mut start = close;
    while start > 0 && bytes[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    Some(start)
}

/// `ternary_condition?(node) && next_char_is_question_mark?(node)`.
fn is_ternary_condition_before_question_mark(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "conditional" {
        return false;
    }
    // The `?` sits where the closing parenthesis ended, so removing it would join the two.
    context
        .source
        .text()
        .as_bytes()
        .get(node.end_byte())
        .is_some_and(|byte| *byte == b'?')
}

/// `comment_above_close_paren_swallows_chain?`.
fn comment_above_close_paren(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(last) = super::nodes::children(node).last().copied() else {
        return false;
    };
    let close = node.end_byte() - 1;
    if last.end_byte() >= close {
        return false;
    }
    let between = context.source.slice(last.end_byte()..close);
    if !between
        .split_inclusive('\n')
        .any(|line| line.contains('#') && line.ends_with('\n'))
    {
        return false;
    }
    // `chained_after_close_paren?`: something other than a comment follows the `)` on its line.
    let (line_number, column) = context.source.line_column(close);
    let line = context.source.line(line_number);
    let after: String = line.chars().skip(column).collect();
    let trimmed = after.trim_start().trim_end_matches(['\n', '\r']);
    !trimmed.is_empty() && !trimmed.starts_with('#')
}
