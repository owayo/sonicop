use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((unpack, format)) = first_element_of_unpack(context, node) else {
            continue;
        };
        let Some(selector) = unpack.field("method") else {
            continue;
        };
        let range = selector.start_byte()..node.end_byte();
        let message = format!(
            "Use `unpack1({})` instead of `{}`.",
            context.source.node_text(format),
            &context.source.text()[range.clone()]
        );
        offenses.push(
            context
                .offense(message, range)
                .corrected_by_all([
                    Edit {
                        start: unpack.end_byte(),
                        end: node.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: selector.start_byte(),
                        end: selector.end_byte(),
                        replacement: "unpack1".to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `(call $(call (...) :unpack $(...)) :first)` and the indexed spellings of the same thing.
fn first_element_of_unpack<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let (receiver, taken_first) = match node.kind_str() {
        // `x.unpack('h*')[0]` is a call to `:[]` upstream.
        "element_reference" => {
            let object = node.field("object")?;
            let indices = super::nodes::children(node);
            let index = match indices.as_slice() {
                [_, only] => *only,
                _ => return None,
            };
            (object, context.source.node_text(index) == "0")
        }
        _ => {
            if node.field("block").is_some() {
                return None;
            }
            let receiver = node.field("receiver")?;
            let method = node.field("method")?;
            let arguments = node
                .field("arguments")
                .map(super::nodes::children)
                .unwrap_or_default();
            let taken = match (context.source.node_text(method), arguments.as_slice()) {
                ("first", []) => true,
                ("slice" | "at", [index]) => context.source.node_text(*index) == "0",
                _ => return None,
            };
            (receiver, taken)
        }
    };
    if !taken_first || receiver.kind_str() != "call" || receiver.field("block").is_some() {
        return None;
    }
    // `(...)`: the receiver of `unpack` has to be a node with children, which every literal and
    // call written here is.
    receiver.field("receiver")?;
    let method = receiver.field("method")?;
    if context.source.node_text(method) != "unpack" {
        return None;
    }
    let arguments = receiver.field("arguments")?;
    match super::nodes::children(arguments).as_slice() {
        [only] => Some((receiver, *only)),
        _ => None,
    }
}
