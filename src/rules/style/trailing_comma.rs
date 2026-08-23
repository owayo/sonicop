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
    node: Node<'_>,
    items: &[Node<'_>],
    kind: &str,
    begin_pos: usize,
    end_pos: usize,
    offenses: &mut Vec<Offense>,
) {
    let style: String = context
        .setting("EnforcedStyleForMultiline")
        .unwrap_or_else(|| "no_comma".to_owned());
    if begin_pos > end_pos {
        return;
    }
    let source = &context.source.text()[begin_pos..end_pos];
    let offset = comma_offset(source, items.iter().any(|item| holds_heredoc(*item)))
        .filter(|offset| !inside_comment(context, begin_pos, begin_pos + offset));

    match offset {
        // `check_comma`: a comma is there, and only a style that wants one leaves it alone.
        Some(offset) => {
            if should_have_comma(context, &style, node, items) {
                return;
            }
            avoid_comma(context, kind, &style, begin_pos + offset, offenses);
        }
        // `put_comma`: no comma, and the style asks for one.
        None => {
            if !should_have_comma(context, &style, node, items) {
                return;
            }
            put_comma(context, kind, items, offenses);
        }
    }
}

/// `should_have_comma?`.
///
/// `diff_comma` is not reachable from the shipped configuration -- it is not among the supported
/// styles of any of the four cops -- so it is left out rather than guessed at.
fn should_have_comma(
    context: &RuleContext<'_>,
    style: &str,
    node: Node<'_>,
    items: &[Node<'_>],
) -> bool {
    match style {
        "comma" => multiline(context, node, items) && no_elements_on_same_line(node, items),
        "consistent_comma" => multiline(context, node, items),
        "diff_comma" => {
            multiline(context, node, items) && last_item_precedes_newline(context, node, items)
        }
        _ => false,
    }
}

/// `multiline?`: written across lines, except for the single argument whose closing bracket shares
/// the last line of the argument itself.
fn multiline(context: &RuleContext<'_>, node: Node<'_>, items: &[Node<'_>]) -> bool {
    if node.start_position().row == node.end_position().row {
        return false;
    }
    // `allowed_multiline_argument?`
    !(items.len() == 1 && !begins_its_line(context, node.end_byte().saturating_sub(1)))
}

/// `no_elements_on_same_line?`: no item ends on the line the next one starts, and none ends on the
/// line the closing bracket sits.
///
/// Rows come from `Position`, which is 0-based; `SourceFile::line_column` is 1-based. Mixing the
/// two silently shifts every comparison by one -- it did, and the cop reported the opposite.
fn no_elements_on_same_line(node: Node<'_>, items: &[Node<'_>]) -> bool {
    let mut rows: Vec<(usize, usize)> = items
        .iter()
        .map(|item| (item.start_position().row, item.end_position().row))
        .collect();
    let closing = node.end_position().row;
    rows.push((closing, closing));
    rows.windows(2).all(|pair| pair[0].1 != pair[1].0)
}

/// `last_item_precedes_newline?`: `/,?\s*(#.*)?\n/` -- at most a comma, blanks and a comment stand
/// between the last item and the end of its line.
fn last_item_precedes_newline(
    context: &RuleContext<'_>,
    node: Node<'_>,
    items: &[Node<'_>],
) -> bool {
    let Some(last) = items.last() else {
        return false;
    };
    let text = &context.source.text()[last.end_byte()..node.end_byte()];
    let rest = text.strip_prefix(',').unwrap_or(text);
    // `\s*` would swallow the newline, but the pattern needs one after it, so blanks stop short of
    // the line break.
    let rest = rest.trim_start_matches([' ', '\t', '\r', '\x0b', '\x0c']);
    let rest = match rest.starts_with('#') {
        true => rest.find('\n').map_or("", |index| &rest[index..]),
        false => rest,
    };
    rest.starts_with('\n')
}

/// `Util.begins_its_line?`: the byte is the first non-blank of its line.
fn begins_its_line(context: &RuleContext<'_>, offset: usize) -> bool {
    let (line, column) = context.source.line_column(offset);
    let text = context.source.line(line);
    text.find(|character: char| !character.is_whitespace())
        .is_some_and(|first| text[..first].chars().count() + 1 == column)
}

/// `extra_avoid_comma_info`.
fn extra_avoid_comma_info(style: &str) -> &'static str {
    match style {
        "comma" => ", unless each item is on its own line",
        "consistent_comma" => ", unless items are split onto multiple lines",
        _ => "",
    }
}

/// `avoid_comma`.
fn avoid_comma(
    context: &RuleContext<'_>,
    kind: &str,
    style: &str,
    comma: usize,
    offenses: &mut Vec<Offense>,
) {
    let article = match kind.contains("array") {
        true => "an",
        false => "a",
    };
    offenses.push(
        context
            .offense(
                format!(
                    "Avoid comma after the last {}{}.",
                    kind.replace("%<article>s", article),
                    extra_avoid_comma_info(style)
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

/// `put_comma`: the offense sits on the last item, not on the missing comma.
fn put_comma(
    context: &RuleContext<'_>,
    kind: &str,
    items: &[Node<'_>],
    offenses: &mut Vec<Offense>,
) {
    let Some(last) = items.last() else {
        return;
    };
    if last.kind_str() == "block_pass" {
        return;
    }
    let range = autocorrect_range(context, *last);
    offenses.push(
        context
            .offense(
                format!(
                    "Put a comma after the last {}.",
                    kind.replace("%<article>s", "a multiline")
                ),
                range.clone(),
            )
            .corrected_by(Edit {
                start: range.end,
                end: range.end,
                replacement: ",".to_owned(),
                safe: true,
            }),
    );
}

/// `autocorrect_range`: the last line of the item, from its first non-blank character.
fn autocorrect_range(context: &RuleContext<'_>, item: Node<'_>) -> std::ops::Range<usize> {
    let text = context.source.node_text(item);
    let after_newline = text.rfind('\n').map_or(0, |index| index + 1);
    let indent = text[after_newline..]
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(0);
    (item.start_byte() + after_newline + indent)..item.end_byte()
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
