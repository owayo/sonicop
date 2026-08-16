//! `ParenthesesCorrector`: taking a pair of parentheses out of the source.

use tree_sitter::Node;

use crate::diagnostic::Edit;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

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
    // `remove_close_paren`: `range_with_surrounding_space(side: :left, newlines: newlines)`.
    // `whitespace` and `continuations` both stay at their default of `false`, so the walk stops at
    // a `\` that ends the line and leaves it standing. The `continuations: true` walk belongs to
    // `parens_range`, which is a *different* range -- see `orphaned_comma_start` below.
    let mut close_start = super::ranges::extended_left(text, close, newlines);
    // `handle_orphaned_comma`: a comma left alone on its own line would not parse, so the
    // whitespace before it goes as well.
    //
    // Upstream removes this as a *second* range, and it always contains the one above, so its
    // `TreeRewriter` folds the pair into the wider removal. Handing two edits over separately
    // loses the whole offense instead, and the place it goes is not the engine: this cop verifies
    // its own correction by reparsing, and `apply_edits` walks the edits in order and refuses any
    // that starts before the previous one ended. Two removals of the same span are the commonest
    // way to trip that, since the walks above land on the same byte whenever no continuation sits
    // between them. Widening the single edit reaches the same text and stays verifiable.
    //
    // The heredoc half of upstream's handling is left out -- nothing in the corpora reaches it,
    // and it rewrites the comma rather than removing it.
    if let Some(orphaned) = orphaned_comma_start(context, close) {
        close_start = close_start.min(orphaned);
    }
    let mut edits = vec![
        Edit {
            start: range.start,
            end: open_end,
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: close_start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        },
    ];
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
///
/// This went unreached for a while, and not because the shape is rare: returning `Some` here used
/// to add a *second* edit overlapping the one for the closing parenthesis, and the engine dropped
/// the offense rather than merging the pair. The cop looked like it could not detect the shape
/// while the cause sat in the corrector. Folding the two into one edit above fixed both, and the
/// `continuations` step below then started doing its work -- it is what keeps the `\` from being
/// left in front of the comma.
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
    // continuations: true)`. The `continuations` step is the one that matters: a `\` ending the
    // line would otherwise be left behind once the whitespace around it goes, stranding the
    // backslash in front of the comma and producing source that does not parse.
    Some(crate::rules::support::final_pos(
        context.source.text(),
        close,
        false,
        true,
        true,
        true,
    ))
}

/// `ternary_condition?(node) && next_char_is_question_mark?(node)`.
fn is_ternary_condition_before_question_mark(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    if parent.kind_str() != "conditional" {
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
