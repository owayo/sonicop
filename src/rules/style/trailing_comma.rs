//! The shared half of `Style/TrailingCommaIn*`, which upstream keeps in `mixin/trailing_comma.rb`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Reports the comma standing between the last item and the closing bracket, when the configured
/// style is one that wants none there.
///
/// `begin_pos` is the end of the last item and `end_pos` the start of the closing bracket, exactly
/// as upstream's `check` is called.
pub(super) fn check(
    context: &RuleContext<'_>,
    items: &[Node<'_>],
    kind: &str,
    begin_pos: usize,
    end_pos: usize,
    offenses: &mut Vec<Offense>,
) {
    let style: String = context
        .setting("EnforcedStyleForMultiline")
        .unwrap_or_else(|| "no_comma".to_owned());
    // Only `no_comma` ever asks for the comma to go; every other style wants one added, which is a
    // separate report this cop does not reach with the default configuration.
    if style != "no_comma" || begin_pos > end_pos {
        return;
    }
    let source = &context.source.text()[begin_pos..end_pos];
    let Some(offset) = comma_offset(source, items.iter().any(|item| holds_heredoc(*item))) else {
        return;
    };
    if inside_comment(context, begin_pos, begin_pos + offset) {
        return;
    }

    let comma = begin_pos + offset;
    let article = match kind.contains("array") {
        true => "an",
        false => "a",
    };
    offenses.push(
        context
            .offense(
                format!(
                    "Avoid comma after the last {}.",
                    kind.replace("%<article>s", article)
                ),
                comma..comma + 1,
            )
            .corrected_by(Edit {
                start: comma,
                end: comma + 1,
                replacement: String::new(),
                safe: true,
            }),
    );
}

/// `comma_offset`: where the comma sits, when only blanks stand between it and the last item.
///
/// A heredoc among the items moves its body between the two positions, so newlines stop counting as
/// blanks there and a comma opening a body line is not the literal's own.
fn comma_offset(source: &str, any_heredoc: bool) -> Option<usize> {
    // Ruby's `\s` is the ASCII set alone, so a non-breaking space is not a blank here.
    let blanks: &[char] = match any_heredoc {
        true => &[' ', '\t', '\r', '\x0c', '\x0b'],
        false => &[' ', '\t', '\r', '\n', '\x0c', '\x0b'],
    };
    let leading = source
        .find(|character: char| !blanks.contains(&character))
        .unwrap_or(source.len());
    if source[leading..].starts_with(',') {
        source.find(',')
    } else {
        None
    }
}

/// `inside_comment?`: a comment opening on the line the search starts on swallows the comma.
fn inside_comment(context: &RuleContext<'_>, start: usize, comma: usize) -> bool {
    let (line, _) = context.source.line_column(start);
    let range = context.source.line_range(line);
    context
        .comment_ranges()
        .iter()
        .rfind(|comment| comment.start >= range.start && comment.start < range.end)
        .is_some_and(|comment| comment.start < comma)
}

/// `heredoc?`: the item is a heredoc, or ends in one through a call or a hash value.
fn holds_heredoc(node: Node<'_>) -> bool {
    match node.kind_str() {
        "heredoc_beginning" => true,
        "call" => match last_argument(node) {
            // `(send receiver method)` has two children upstream, so a call without arguments looks
            // at its receiver and one with arguments at its last argument.
            Some(argument) => holds_heredoc(argument),
            None => node.field("receiver").is_some_and(holds_heredoc),
        },
        "pair" | "hash" => super::nodes::children(node)
            .last()
            .is_some_and(|last| holds_heredoc(*last)),
        _ => false,
    }
}

fn last_argument<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    super::nodes::children(node.field("arguments")?)
        .last()
        .copied()
}

/// The closing bracket of a literal, which is where the range upstream searches ends.
pub(super) fn closing_bracket<'tree>(node: Node<'tree>, bracket: &str) -> Option<Node<'tree>> {
    let last = node.child(node.child_count().saturating_sub(1) as u32)?;
    (last.kind_str() == bracket).then_some(last)
}
