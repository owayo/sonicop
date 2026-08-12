use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_BRACES: &str = "Wrap hash in `{` and `}`.";
const MSG_NO_BRACES: &str = "Omit the braces around the hash.";

/// One element of an array as upstream's parser groups them: the trailing key-value pairs are one
/// `hash` node there, where the grammar leaves them as siblings.
struct Value<'tree> {
    range: Range<usize>,
    /// The node itself, for an element the grammar spells out.
    node: Option<Node<'tree>>,
    is_hash: bool,
    braces: bool,
    /// `node.children.first&.kwsplat_type?`.
    opens_with_splat: bool,
    empty: bool,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let braces_style = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "braces");

    for array in context.nodes_of("array") {
        // `explicit_array?`: `%w[...]` opens with a percent literal, and an implicit array has no
        // brackets at all to put a hash in.
        if !context.source.node_text(array).starts_with('[') {
            continue;
        }
        let values = values(context, array);
        let Some(last) = values.last() else {
            continue;
        };
        if !last.is_hash || last.opens_with_splat {
            continue;
        }
        // `expected_braced_last_array_item?`: an array of nothing but hashes already written the
        // way this style wants is left alone, and so is one whose last two items are both hashes.
        if values
            .iter()
            .all(|value| value.is_hash && value.braces == braces_style)
        {
            continue;
        }
        if values[..values.len() - 1]
            .last()
            .is_some_and(|previous| previous.is_hash)
        {
            continue;
        }

        match braces_style {
            true if !last.braces => offenses.push(wrap(context, array, last)),
            false if last.braces && !last.empty => offenses.push(unwrap(context, array, last)),
            _ => {}
        }
    }
}

/// `check_braces`: the pairs gain the braces they were written without.
fn wrap(context: &RuleContext<'_>, array: Node<'_>, last: &Value<'_>) -> Offense {
    let (start_row, start_column) = position(context, last.range.start);
    let (end_row, _) = position(context, last.range.end);
    let one_line = start_row == end_row || start_row == array.start_position().row;
    let (before, after) = match one_line {
        true => ("{".to_owned(), "}".to_owned()),
        false => {
            let indent = " ".repeat(start_column);
            (format!("{{\n{indent}"), format!("\n{indent}}}"))
        }
    };
    context
        .offense(MSG_BRACES, last.range.clone())
        .corrected_by_all([
            Edit {
                start: last.range.start,
                end: last.range.start,
                replacement: before,
                safe: true,
            },
            Edit {
                start: last.range.end,
                end: last.range.end,
                replacement: after,
                safe: true,
            },
        ])
}

/// `check_no_braces`: the braces come off, and with them a comma that would now separate the
/// array's own items.
fn unwrap(context: &RuleContext<'_>, array: Node<'_>, last: &Value<'_>) -> Offense {
    let mut edits = Vec::new();
    if let Some(comma) = trailing_comma(context, last.range.end) {
        edits.push(Edit {
            start: comma,
            end: comma + 1,
            replacement: String::new(),
            safe: true,
        });
    }
    let _ = array;
    if let Some(node) = last.node
        && let (Some(open), Some(close)) = (
            node.child(0),
            node.child(node.child_count().saturating_sub(1) as u32),
        )
    {
        edits.push(Edit {
            start: open.start_byte(),
            end: open.end_byte(),
            replacement: String::new(),
            safe: true,
        });
        edits.push(Edit {
            start: close.start_byte(),
            end: close.end_byte(),
            replacement: String::new(),
            safe: true,
        });
    }
    context
        .offense(MSG_NO_BRACES, last.range.clone())
        .corrected_by_all(edits)
}

/// `remove_last_element_trailing_comma`: the first thing after the last element, when it is a
/// comma.
fn trailing_comma(context: &RuleContext<'_>, end: usize) -> Option<usize> {
    let text = context.source.text();
    let offset = text[end..]
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .map(|(offset, _)| end + offset)?;
    (text.as_bytes().get(offset) == Some(&b',')).then_some(offset)
}

/// `array.values`: the elements, with the trailing key-value pairs folded into one hash.
fn values<'tree>(context: &RuleContext<'_>, array: Node<'tree>) -> Vec<Value<'tree>> {
    let children = super::nodes::children(array);
    let split = children.iter().position(|child| is_pair(*child));
    let (elements, pairs) = match split {
        Some(index) => children.split_at(index),
        None => (children.as_slice(), &[][..]),
    };

    let mut values: Vec<Value<'tree>> = elements
        .iter()
        .map(|element| Value {
            range: element.byte_range(),
            node: Some(*element),
            is_hash: element.kind() == "hash",
            braces: element.kind() == "hash",
            opens_with_splat: element.kind() == "hash"
                && super::nodes::children(*element)
                    .first()
                    .is_some_and(|first| first.kind() == "hash_splat_argument"),
            empty: element.kind() == "hash" && super::nodes::children(*element).is_empty(),
        })
        .collect();
    let _ = context;
    if let (Some(first), Some(last)) = (pairs.first(), pairs.last()) {
        values.push(Value {
            range: first.start_byte()..last.end_byte(),
            node: None,
            is_hash: true,
            braces: false,
            opens_with_splat: first.kind() == "hash_splat_argument",
            empty: false,
        });
    }
    values
}

fn is_pair(node: Node<'_>) -> bool {
    matches!(node.kind(), "pair" | "hash_splat_argument")
}

fn position(context: &RuleContext<'_>, offset: usize) -> (usize, usize) {
    let (line, column) = context.source.line_column(offset);
    (line - 1, column - 1)
}
