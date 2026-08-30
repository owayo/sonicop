//! `Style/ConcatArrayLiterals`: `push` takes the elements, so wrapping them in an array first is
//! an array nobody keeps.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node
            .field("method")
            .is_none_or(|name| context.source.node_text(name) != "concat")
        {
            continue;
        }
        let list = arguments(node);
        if list.is_empty() {
            continue;
        }
        let Some(arrays) = list
            .iter()
            .map(|argument| {
                (argument.parts().len() == 1)
                    .then(|| ArrayKind::of(argument.first()).map(|kind| (kind, argument.first())))
                    .flatten()
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        // `offense_range`: the selector through the end of the call, so the receiver stays put.
        let Some(selector) = node.field("method") else {
            continue;
        };
        let range = selector.start_byte()..node.end_byte();
        let current = context.source.slice(range.clone());
        let percent = arrays
            .iter()
            .any(|(kind, _)| !matches!(kind, ArrayKind::Bracketed));
        let preferred = preferred_method(&arrays, context);
        let message = match (percent, &preferred) {
            (true, None) => format!(
                "Use `push` with elements as arguments without array brackets instead of \
                 `{current}`."
            ),
            (_, prefer) => {
                let prefer = prefer.as_deref().unwrap_or_default();
                format!("Use `{prefer}` instead of `{current}`.")
            }
        };
        let offense = context.offense(message, range.clone());
        // Three corrections upstream, picked in this order: a percent literal or an empty array
        // has to be rewritten whole, and anything else keeps its arguments and only loses the
        // brackets around them.
        let whole = percent
            || arrays
                .iter()
                .any(|(_, array)| super::nodes::children_in(*array, context).is_empty());
        let edits = if whole {
            match preferred {
                Some(prefer) => vec![replace(range, prefer)],
                None => Vec::new(),
            }
        } else {
            let mut edits = vec![replace(
                selector.start_byte()..selector.end_byte(),
                "push".to_owned(),
            )];
            let bracketed = arrays.iter().all(|(_, array)| match brackets(*array) {
                Some((open, close)) => {
                    edits.push(replace(open, String::new()));
                    edits.push(replace(close, String::new()));
                    true
                }
                None => false,
            });
            if bracketed { edits } else { Vec::new() }
        };
        offenses.push(if edits.is_empty() {
            offense
        } else {
            offense.corrected_by_all(edits)
        });
    }
}

fn replace(range: std::ops::Range<usize>, replacement: String) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement,
        safe: true,
    }
}

/// `preferred_method`: every element of every argument, spelled as one `push` call.
///
/// A percent literal's elements are written bare, so upstream re-inspects their values; anything
/// but a string or a symbol has no value to inspect, and the cop reports without correcting.
fn preferred_method(arrays: &[(ArrayKind, Node<'_>)], context: &RuleContext<'_>) -> Option<String> {
    let mut elements: Vec<String> = Vec::new();
    for (kind, array) in arrays {
        for element in super::nodes::children_in(*array, context) {
            match kind {
                ArrayKind::Bracketed => elements.push(context.source.node_text(element).to_owned()),
                _ => {
                    if super::nodes::children_in(element, context)
                        .iter()
                        .any(|part| part.kind_str() == "interpolation")
                    {
                        return None;
                    }
                    let value = super::literal::node_value(context, element)?;
                    elements.push(match kind {
                        ArrayKind::Strings => super::literal::inspect_string(&value.value),
                        _ => super::literal::inspect_symbol(&value.value),
                    });
                }
            }
        }
    }
    Some(format!("push({})", elements.join(", ")))
}

/// The `[` and the `]` of an array literal, which the correction drops.
fn brackets(node: Node<'_>) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let open = node.child(0)?;
    let close = node.child(node.child_count().checked_sub(1)? as u32)?;
    (open.id() != close.id()).then(|| (open.byte_range(), close.byte_range()))
}

/// The three spellings of an `array` node, which upstream tells apart with `percent_literal?`.
#[derive(Clone, Copy)]
enum ArrayKind {
    Bracketed,
    Strings,
    Symbols,
}

impl ArrayKind {
    fn of(node: Node<'_>) -> Option<Self> {
        match node.kind_str() {
            "array" => Some(Self::Bracketed),
            "string_array" => Some(Self::Strings),
            "symbol_array" => Some(Self::Symbols),
            _ => None,
        }
    }
}
