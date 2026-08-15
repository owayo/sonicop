use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const MSG: &str = "Remove the redundant `Array` constructor.";

/// The three constructors that only wrap an array literal:
///
/// * `(send (const {nil? cbase} :Array) :new $(array ...))`
/// * `(send (const {nil? cbase} :Array) :[] $...)`
/// * `(send nil? :Array $(array ...))`
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((reported, replacement)) = redundant(node, context) else {
            continue;
        };
        // `corrector.replace(node, ...)`: the send, so a block written after it stays.
        let replaced = send_node::send_range(node, context);
        offenses.push(context.offense(MSG, reported).corrected_by(Edit {
            start: replaced.start,
            end: replaced.end,
            replacement,
            safe: true,
        }));
    }
}

/// The reported range and the text the call collapses to.
fn redundant(
    node: tree_sitter::Node<'_>,
    context: &RuleContext<'_>,
) -> Option<(std::ops::Range<usize>, String)> {
    // `Array[...]` is a call to `:[]` upstream, but an `element_reference` here.
    if node.kind_str() == "element_reference" {
        let object = node.field("object")?;
        if !send_node::top_level_constant(object, "Array", context) {
            return None;
        }
        // `range = receiver` and the replacement runs from the opening bracket to the end, so the
        // subscript is reused verbatim as an array literal.
        let brackets = context.source.text()[object.end_byte()..node.end_byte()].to_owned();
        return Some((object.byte_range(), brackets));
    }
    let selector = node.field("method")?;
    let name = context.source.node_text(selector);
    let arguments = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    match (node.field("receiver"), name) {
        // `Array.new([1, 2])`: the report covers `Array.new`, which is what becomes the literal.
        (Some(receiver), "new") if send_node::top_level_constant(receiver, "Array", context) => {
            let [only] = arguments.as_slice() else {
                return None;
            };
            if only.kind_str() != "array" {
                return None;
            }
            Some((
                receiver.start_byte()..selector.end_byte(),
                context.source.node_text(*only).to_owned(),
            ))
        }
        // `Array([1, 2])`: `Kernel#Array` written without a receiver.
        (None, "Array") => {
            let [only] = arguments.as_slice() else {
                return None;
            };
            if only.kind_str() != "array" {
                return None;
            }
            Some((
                selector.byte_range(),
                context.source.node_text(*only).to_owned(),
            ))
        }
        _ => None,
    }
}
