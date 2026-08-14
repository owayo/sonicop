use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::directives::DirectiveState;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::ruby_version::RubyVersion;

use super::statements::statements;

const MSG: &str = "Avoid `in` branches without a body.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < RubyVersion::new(2, 7) {
        return;
    }
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    for case in context.nodes_of("case_match") {
        let mut cursor = case.walk();
        let children: Vec<Node<'_>> = case.named_children(&mut cursor).collect();
        for (index, &branch) in children.iter().enumerate() {
            if branch.kind_str() != "in_clause" || has_body(branch) {
                continue;
            }
            if allow_comments && allowed_by_comments(context, case, &children, index) {
                continue;
            }
            offenses.push(context.offense(MSG, reported_range(branch)));
        }
    }
}

/// The span upstream reports. An `in_pattern` with no body ends at its guard, or at its pattern
/// when it has none, so the `then` written before the body it does not have is no part of it.
fn reported_range(branch: Node<'_>) -> Range<usize> {
    let end = branch
        .field("guard")
        .or_else(|| branch.field("pattern"))
        .map_or_else(|| branch.end_byte(), |node| node.end_byte());
    branch.start_byte()..end
}

fn has_body(branch: Node<'_>) -> bool {
    branch
        .field("body")
        .is_some_and(|body| !statements(body).is_empty())
}

/// `allow_comments?`: a branch that explains itself is left alone, unless the only thing covering
/// it is the directive that turns this cop off.
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

/// `find_end_line`: a branch runs up to the line the next one starts on, or to the `end` of the
/// `case` when it is the last. The range excludes that line.
fn comment_search_lines(
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
                let (line, _) = context
                    .source
                    .line_column(case.end_byte().saturating_sub(1));
                line
            },
            |next| context.source.line_column(next.start_byte()).0,
        );
    start..end
}

fn comments_contain_disables(context: &RuleContext<'_>, lines: &Range<usize>) -> bool {
    let directives = DirectiveState::parse(context.source, context.comment_ranges());
    directives
        .disabled_line_ranges("Lint/EmptyInPattern", context.source)
        .iter()
        .any(|disabled| {
            (disabled.start <= lines.start && lines.end.saturating_sub(1) <= disabled.end)
                || (lines.start <= disabled.start && disabled.end < lines.end)
        })
}
