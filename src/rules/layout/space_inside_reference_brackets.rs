//! `Layout/SpaceInsideReferenceBrackets`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "no_space".to_owned());
    let empty_style: String = context
        .setting("EnforcedStyleForEmptyBrackets")
        .unwrap_or_else(|| "no_space".to_owned());
    let text = context.source.text();

    for node in context.nodes_of("element_reference") {
        // `a[0] = 1` is one `:[]=` send upstream and spans the whole assignment, which is what
        // `multiline?` is asked about. `a[0] += 1` keeps the plain `:[]` send.
        let send = node
            .parent_of(context)
            .filter(|parent| {
                parent.kind_str() == "assignment" && parent.field("left") == Some(node)
            })
            .unwrap_or(node);
        let is_setter = send.id() != node.id();
        let Some((left, right)) = left_ref_bracket(node, is_setter, context) else {
            continue;
        };
        let inner = left.end_byte()..right.start_byte();

        // `empty_brackets?` asks whether the brackets are adjacent in the token stream, so blanks
        // between them still count as empty while a comment does not.
        if text[inner.clone()].trim().is_empty() {
            let range = left.start_byte()..right.end_byte();
            let single_space = inner.len() == 1 && text.as_bytes()[inner.start] == b' ';
            let command = if empty_style == "space" {
                if single_space {
                    continue;
                }
                "Use one"
            } else {
                if inner.is_empty() {
                    continue;
                }
                "Do not use"
            };
            let mut edits = vec![Edit {
                start: inner.start,
                end: inner.end,
                replacement: String::new(),
                safe: true,
            }];
            if empty_style == "space" {
                edits.push(Edit {
                    start: inner.start,
                    end: inner.start,
                    replacement: " ".to_owned(),
                    safe: true,
                });
            }
            offenses.push(
                context
                    .offense(
                        format!("{command} space inside empty reference brackets."),
                        range,
                    )
                    .corrected_by_all(edits),
            );
            continue;
        }

        if send.start_position().row != send.end_position().row {
            continue;
        }
        let mut reported = Vec::new();
        if style == "no_space" {
            if extra_space_after(text, left.end_byte()) {
                reported.push((space_after(text, left.end_byte()), "Do not use"));
            }
            if extra_space_before(text, right.start_byte()) {
                reported.push((space_before(text, right.start_byte()), "Do not use"));
            }
        } else {
            // `space_offense(node, token, :none, ...)`: the offense sits on the bracket itself
            // rather than in the gap the correction fills, so it keeps the bracket's column and
            // its one character of length.
            if !extra_space_after(text, left.end_byte()) {
                reported.push((left.byte_range(), "Use"));
            }
            if !extra_space_before(text, right.start_byte()) {
                reported.push((right.byte_range(), "Use"));
            }
        }
        // Upstream corrects the node from the first offense it reports and calls `ignore_node`, so
        // any later offense on the same node carries no corrector at all.
        let corrections = corrections(text, &style, &left, &right);
        for (index, (range, command)) in reported.into_iter().enumerate() {
            let offense =
                context.offense(format!("{command} space inside reference brackets."), range);
            offenses.push(match index {
                0 => offense.corrected_by_all(corrections.clone()),
                _ => offense,
            });
        }
    }
}

/// `SpaceCorrector.remove_space` / `add_space`: both bracket sides are rewritten together, so a
/// side that was excused from reporting still gets corrected.
fn corrections(text: &str, style: &str, left: &Node<'_>, right: &Node<'_>) -> Vec<Edit> {
    let mut edits = Vec::new();
    if style == "no_space" {
        if has_space_after(text, left.end_byte()) {
            let range = space_after(text, left.end_byte());
            if !range.is_empty() {
                edits.push(remove(range));
            }
        }
        if has_space_before(text, right.start_byte()) {
            let range = space_before(text, right.start_byte());
            if !range.is_empty() {
                edits.push(remove(range));
            }
        }
    } else {
        if !has_space_after(text, left.end_byte()) {
            edits.push(insert(left.end_byte()));
        }
        if !has_space_before(text, right.start_byte()) {
            edits.push(insert(right.start_byte()));
        }
    }
    edits
}

/// `left_ref_bracket` and the `closing_bracket` that answers it: which pair of brackets this send
/// is held to.
///
/// Upstream picks out of `tokens_within(node)` -- every token the send spans, children included --
/// rather than out of the node's own brackets, and then applies a rule that usually sends it back
/// to the *first* pair:
///
/// ```ruby
/// a['x'].b['y']   # the outer send spans both pairs; the token before `['y']` is `b`, not `]`,
///                 # so it reports on `['x']` -- which the inner send reported already.
/// a['x']['y']     # the token before `['y']` is `]`, so the outer send reports on its own pair.
/// ```
///
/// The effect is that a pair reached through a method call is reported once, not twice, and the
/// *outer* pair of such a chain is never reported at all. Reading each node's own brackets instead
/// reports every pair, which is 36 offenses too many on rubocop/rubocop under `EnforcedStyle:
/// space`.
fn left_ref_bracket<'tree>(
    node: Node<'tree>,
    is_setter: bool,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let opens = index_brackets(node);
    let last = *opens.last()?;
    // `previous_token(current_token).right_bracket?`: what stands immediately before the bracket.
    let closes_a_bracket = context.source.text()[..last.start_byte()]
        .trim_end()
        .ends_with(']');
    let chosen = if is_setter || !closes_a_bracket {
        *opens.first()?
    } else {
        last
    };
    let close = chosen
        .parent()
        .and_then(|parent| brackets(parent).1)
        .or_else(|| brackets(node).1)?;
    Some((chosen, close))
}

/// Every index bracket the node spans, in source order. An array literal's `[` is a different
/// token type upstream (`tLBRACK` against `tLBRACK2`) and is not one of these.
fn index_brackets<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut opens = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "element_reference"
            && let Some(open) = brackets(current).0
        {
            opens.push(open);
        }
        let mut cursor = current.walk();
        stack.extend(current.children(&mut cursor));
    }
    opens.sort_by_key(tree_sitter::Node::start_byte);
    opens
}

/// The index's own brackets. Nested and chained ones belong to child nodes, which is what
/// `left_ref_bracket` picks out of the token run.
fn brackets<'tree>(node: Node<'tree>) -> (Option<Node<'tree>>, Option<Node<'tree>>) {
    let mut cursor = node.walk();
    let mut left = None;
    let mut right = None;
    for child in node.children(&mut cursor) {
        match child.kind_str() {
            "[" if left.is_none() => left = Some(child),
            "]" => right = Some(child),
            _ => {}
        }
    }
    (left, right)
}

fn insert(offset: usize) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement: " ".to_owned(),
        safe: true,
    }
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// `extra_space?`, which only a space or a tab satisfies.
fn extra_space_after(text: &str, offset: usize) -> bool {
    matches!(text.as_bytes().get(offset), Some(b' ' | b'\t'))
}

fn extra_space_before(text: &str, offset: usize) -> bool {
    offset > 0 && matches!(text.as_bytes().get(offset - 1), Some(b' ' | b'\t'))
}

/// `token.space_after?`, which any whitespace satisfies.
fn has_space_after(text: &str, offset: usize) -> bool {
    text[offset..].starts_with(char::is_whitespace)
}

fn has_space_before(text: &str, offset: usize) -> bool {
    let probe = if offset == 0 { 0 } else { offset - 1 };
    text[probe..].starts_with(char::is_whitespace)
}

fn space_after(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = offset;
    while matches!(bytes.get(end), Some(b' ' | b'\t')) {
        end += 1;
    }
    offset..end
}

fn space_before(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}
