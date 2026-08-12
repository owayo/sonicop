use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some(found) = redundant_sort(context, node) else {
            continue;
        };
        let Some(selector) = found.sort.child_by_field_name("method") else {
            continue;
        };
        let suggestion = format!(
            "{}{}",
            match found.wants_first {
                true => "min",
                false => "max",
            },
            match found.sorter {
                "sort_by" => "_by",
                _ => "",
            }
        );
        // `accessor_source`: from the accessor's own selector to the end of the expression.
        let accessor_source = &context.source.text()[found.accessor_start..node.end_byte()];
        let message = format!(
            "Use `{suggestion}` instead of `{}...{accessor_source}`.",
            found.sorter
        );
        let mut edits = vec![
            Edit {
                start: found.removed_from,
                end: node.end_byte(),
                replacement: String::new(),
                safe: true,
            },
            Edit {
                start: selector.start_byte(),
                end: selector.end_byte(),
                replacement: suggestion,
                safe: true,
            },
        ];
        // `replace_with_logical_operator`: the operator moves to just after the sorting call,
        // because the text between the two is about to go.
        if let Some(operator) = logical_operator(context, node) {
            edits.push(Edit {
                start: found.sort.end_byte(),
                end: found.sort.end_byte(),
                replacement: format!(" {}", context.source.node_text(operator)),
                safe: true,
            });
            edits.push(Edit {
                start: operator.start_byte(),
                end: operator.end_byte(),
                replacement: String::new(),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(message, selector.start_byte()..node.end_byte())
                .corrected_by_all(edits)
                // The inserted operator hangs off the sorting call, not off what was reported.
                .corrections_anchored_at(found.sort.byte_range()),
        );
    }
}

struct Found<'tree> {
    /// The `sort` or `sort_by` call whose selector becomes `min` / `max`.
    sort: Node<'tree>,
    sorter: &'static str,
    wants_first: bool,
    /// Where the accessor's *selector* starts, which is what the message quotes.
    accessor_start: usize,
    /// Where the whole accessor starts, dot included, which is what the correction removes.
    removed_from: usize,
}

fn redundant_sort<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Found<'tree>> {
    let (receiver, wants_first, accessor_start, removed_from) = match node.kind() {
        // `sorted[0]` is a call to `:[]` upstream, whose selector is the bracket.
        "element_reference" => {
            let object = node.child_by_field_name("object")?;
            let indices = super::nodes::children(node);
            let index = match indices.as_slice() {
                [_, only] => *only,
                _ => return None,
            };
            let wants_first = first_or_last(context, index)?;
            let bracket = object.end_byte();
            (object, wants_first, bracket, bracket)
        }
        _ => {
            if node.child_by_field_name("block").is_some() {
                return None;
            }
            let receiver = node.child_by_field_name("receiver")?;
            let method = node.child_by_field_name("method")?;
            let arguments = node
                .child_by_field_name("arguments")
                .map(super::nodes::children)
                .unwrap_or_default();
            let wants_first = match (context.source.node_text(method), arguments.as_slice()) {
                ("first", []) => true,
                ("last", []) => false,
                ("at" | "slice" | "[]", [index]) => first_or_last(context, *index)?,
                _ => return None,
            };
            let dot = node
                .child_by_field_name("operator")
                .map_or_else(|| method.start_byte(), |dot| dot.start_byte());
            (receiver, wants_first, method.start_byte(), dot)
        }
    };
    // The sorting call, which may carry a block of its own.
    let sort = receiver;
    if sort.kind() != "call" {
        return None;
    }
    let method = sort.child_by_field_name("method")?;
    let arguments = sort
        .child_by_field_name("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    let blocked = sort.child_by_field_name("block").is_some();
    let sorter = match context.source.node_text(method) {
        // `sort` takes no arguments of its own; with a block it is an `any_block` upstream.
        "sort" if arguments.is_empty() => "sort",
        // `sort_by` takes exactly one argument, or a block instead.
        "sort_by" if arguments.len() == 1 || (blocked && arguments.is_empty()) => "sort_by",
        _ => return None,
    };
    // Only `sort` written with a block reaches the `any_block` alternatives; a bare `sort` with an
    // argument is not the pattern at all.
    if blocked && sorter == "sort" && !arguments.is_empty() {
        return None;
    }
    Some(Found {
        sort,
        sorter,
        wants_first,
        accessor_start,
        removed_from,
    })
}

/// `with_logical_operator?`: the accessor stands next to an `and` or an `or`.
fn logical_operator<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    if parent.kind() != "binary" {
        return None;
    }
    let operator = parent.child_by_field_name("operator")?;
    matches!(
        context.source.node_text(operator),
        "&&" | "||" | "and" | "or"
    )
    .then_some(operator)
}

/// `{(int 0) (int -1)}`: the first or the last element.
fn first_or_last(context: &RuleContext<'_>, index: Node<'_>) -> Option<bool> {
    match context.source.node_text(index) {
        "0" => Some(true),
        "-1" => Some(false),
        _ => None,
    }
}
