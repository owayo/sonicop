use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::directives::DirectiveState;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::comments_help::comment_search_lines;

const MSG: &str = "Avoid `when` branches without a body.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    for case in context.nodes_of("case") {
        let mut cursor = case.walk();
        let children: Vec<Node<'_>> = case.named_children(&mut cursor).collect();
        for (index, &branch) in children.iter().enumerate() {
            if branch.kind_str() != "when" || has_body(branch) {
                continue;
            }
            if allow_comments && allowed_by_comments(context, case, &children, index) {
                continue;
            }
            offenses.push(context.offense(MSG, reported_range(branch)));
        }
    }
}

/// The span upstream reports. A `when` node ends at its last condition when it has no body, so
/// the `;` or `then` that separates it from the body it does not have is no part of it.
fn reported_range(branch: Node<'_>) -> std::ops::Range<usize> {
    let mut cursor = branch.walk();
    let last = branch
        .named_children(&mut cursor)
        .filter(|child| child.kind_str() == "pattern")
        .last();
    branch.start_byte()..last.map_or_else(|| branch.end_byte(), |last| last.end_byte())
}

/// Whether the branch has a body upstream. tree-sitter keeps the `then` keyword in a body node of
/// its own, so a branch written `when 1 then` still has one here while upstream's `body` is nil.
fn has_body(branch: Node<'_>) -> bool {
    branch
        .field("body")
        .is_some_and(|body| body.named_child_count() > 0)
}

/// `allow_comments?`: a branch that explains itself is left alone -- unless the only thing in it is
/// the directive that turns this cop off, which would otherwise be self-fulfilling.
fn allowed_by_comments(
    context: &RuleContext<'_>,
    case: Node<'_>,
    children: &[Node<'_>],
    index: usize,
) -> bool {
    let lines = comment_search_lines(context, case, children, index);
    let has_comment = context.comment_ranges().iter().any(|comment| {
        let (line, _) = context.source.line_column(comment.start);
        lines.contains(&line)
    });
    has_comment && !comments_contain_disables(context, &lines)
}

/// `comments_contain_disables?`: whether a `rubocop:disable` of this cop overlaps the branch. The
/// directives are read only here, where a branch already turned out to be empty and commented.
fn comments_contain_disables(context: &RuleContext<'_>, lines: &Range<usize>) -> bool {
    let directives = DirectiveState::parse(context.source, context.comment_ranges());
    directives
        .disabled_line_ranges("Lint/EmptyWhen", context.source)
        .iter()
        .any(|disabled| {
            (disabled.start <= lines.start && lines.end.saturating_sub(1) <= disabled.end)
                || (lines.start <= disabled.start && disabled.end < lines.end)
        })
}
