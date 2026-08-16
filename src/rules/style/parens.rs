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
    // The heredoc half of upstream's handling is left out. It rewrites the comma rather than
    // removing it, and no corpus reaches it -- but upstream's own spec does, at
    // `redundant_parentheses_spec.rb:1985` ("an array of multiple heredocs"), so this is a known
    // gap with a case waiting for it rather than dead weight.
    let orphaned = orphaned_comma_start(context, close);
    if let Some(start) = orphaned {
        close_start = close_start.min(start);
    }
    // `extend_range_for_heredoc` and `add_heredoc_comma`: a heredoc's body sits on the lines after
    // the `)`, so a comma left where it was would land in front of that body rather than after the
    // element. Upstream takes the comma with the parentheses and writes a fresh one straight after
    // the opening token, turning `<<-STRING\n...\nSTRING\n) ,` into `<<-STRING,`.
    let heredoc = orphaned.and_then(|_| heredoc_opener(node));
    let close_end = match heredoc {
        Some(_) => comma_after(text, range.end),
        None => range.end,
    };
    let mut edits = vec![
        Edit {
            start: range.start,
            end: open_end,
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: close_start,
            end: close_end,
            replacement: String::new(),
            safe: true,
        },
    ];
    if let Some(opener) = heredoc.filter(|_| close_end > range.end) {
        edits.push(Edit {
            start: opener.end_byte(),
            end: opener.end_byte(),
            replacement: ",".to_owned(),
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

/// `heredoc?`: the group's last element is a heredoc, so pulling the parentheses moves the `)`
/// while the body stays put.
///
/// Upstream asks whether `node.child_nodes.last.loc` is a `Heredoc` map. Here the opening token is
/// its own node and the body is a sibling that trails the whole statement, so the question becomes
/// whether the last child is that opener.
fn heredoc_opener<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    super::nodes::children(node)
        .last()
        .copied()
        .filter(|last| last.kind_str() == "heredoc_beginning")
}

/// `COMMA_REGEXP = /(?<=\))\s*,/` over the line the `)` sits on: how far past the parenthesis the
/// comma reaches, so the removal can take it too.
///
/// The walk stays on the line -- upstream matches inside `range_by_whole_lines`, so a comma opening
/// the next line belongs to something else.
fn comma_after(text: &str, after_close: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = after_close;
    while bytes
        .get(index)
        .is_some_and(|byte| *byte != b'\n' && crate::rules::support::is_ruby_space(*byte))
    {
        index += 1;
    }
    match bytes.get(index) {
        Some(b',') => index + 1,
        _ => after_close,
    }
}

/// `only_closing_paren_before_comma?` with `parens_range`: where the run of whitespace that would
/// leave the comma stranded begins.
///
/// Returning `Some` here used to add a *second* edit for the closing parenthesis, and since the
/// two walks land on the same byte whenever no continuation sits between them, the pair was
/// usually the same removal twice. `apply_edits` refuses an edit that starts before the previous
/// one ended, so the reparse check this cop runs on its own correction failed and took the
/// candidate with it -- the cop looked unable to detect the shape while the cause sat in the
/// corrector. Folding the two into one edit above fixed that, and the `continuations` step below
/// then started doing its work: it is what keeps the `\` from being left in front of the comma.
///
/// **No corpus reaches this** (as of 5 corpora / 18,251 files, 2026-08-17), but upstream's spec
/// does. Across the five corpora the two cops that
/// correct through here fire 1,185 times and none of them is this shape, so a byte comparison of
/// autocorrected output says nothing about the code below. `redundant_parentheses_spec.rb:1966`
/// is exactly it -- `foo(\n  (\n    1\n  ),\n  2\n)` -- so the guard is upstream's own case, not
/// only the ones written here by hand.
///
/// A corpus run agreeing with upstream says only that nothing *else* broke. It does not say this
/// works. `style/nested_parenthesized_calls.rs::leading_space` carries the same note for the same
/// reason.
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
