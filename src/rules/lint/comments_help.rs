//! `CommentsHelp`: the lines a branch owns, which is where a cop looks for the comment that
//! excuses an empty one.
//!
//! Upstream is one mixin included by both `Lint/EmptyWhen` and `Lint/EmptyInPattern`, so the two
//! read the same lines for `when` and for `in`.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `find_end_line`: a `when` runs up to the line the next branch starts on, or to the `end` of the
/// `case` when it is the last. The range excludes that line, which is why the keyword itself never
/// counts as a comment of the branch before it.
pub(super) fn comment_search_lines(
    context: &RuleContext<'_>,
    case: Node<'_>,
    children: &[Node<'_>],
    index: usize,
) -> Range<usize> {
    let branch = children[index];
    let (start, _) = context.source.line_column(branch.start_byte());
    let end = children[index + 1..]
        .iter()
        .find(|sibling| sibling.kind_str() != "comment")
        .map_or_else(
            || {
                // `parent.loc.end.line`, the `end` keyword closing the `case`.
                let (line, _) = context
                    .source
                    .line_column(case.end_byte().saturating_sub(1));
                line
            },
            |next| context.source.line_column(next.start_byte()).0,
        );
    start..end
}
