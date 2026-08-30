use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::all_children_iter;

const MSG: &str = "Put method call on a separate line if chained to a single line block.";

/// The connectors `Send#loc.dot` covers.
const DOTS: &[&str] = &[".", "&.", "::"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `return unless receiver&.any_block_type?`: the call has to be chained onto a block.
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let Some(block) = block_of(receiver) else {
            continue;
        };
        // `return if receiver_location.begin.line < closing_block_delimiter_line_num`: a block
        // spread over several lines is already broken up.
        let closing_line = context.source.line_column(block.end_byte()).0;
        if context.source.line_column(block.start_byte()).0 < closing_line {
            continue;
        }
        let Some(dot) = dot_of(node, receiver, context) else {
            continue;
        };
        // `selector_range`: `l.(1)` has no selector, so the opening parenthesis stands in for it.
        let Some(selector) = node.field("method").or_else(|| opening_parenthesis(node)) else {
            continue;
        };
        let (dot_line, dot_column) = context.source.line_column(dot.start_byte());
        // `call_method_after_block?`: the call was written on the block's own closing line, to the
        // right of the dot.
        if dot_line > closing_line
            || dot_column >= context.source.line_column(selector.start_byte()).1
        {
            continue;
        }
        let range = dot.start_byte()..selector.end_byte();
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.start,
            replacement: "\n".to_owned(),
            safe: true,
        }));
    }
}

/// `node.loc.begin` for a `foo.(1)`: the `(` that opens the argument list, which is all the call
/// has where a name would otherwise be.
fn opening_parenthesis<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let arguments = node.field("arguments")?;
    arguments.child(0).filter(|first| first.kind_str() == "(")
}

/// The `{`/`do` .. `}`/`end` the receiver closes with, when the receiver is a block at all.
///
/// A call written with a block is two nodes upstream -- a `block` wrapped around the `send` -- and
/// one here, so the block is found on the receiver rather than beside it.
fn block_of<'tree>(receiver: Node<'tree>) -> Option<Node<'tree>> {
    match receiver.kind_str() {
        "call" => receiver.field("block"),
        // `-> { }.call` is a `block` upstream too.
        "lambda" => receiver.field("body"),
        _ => None,
    }
}

/// `node.loc.dot`: the connector written between the receiver and the method name.
fn dot_of<'tree>(
    node: Node<'tree>,
    receiver: Node<'_>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let _cursor = node.walk();
    all_children_iter(node, context)
        .filter(|child| !child.is_named() && child.start_byte() >= receiver.end_byte())
        .find(|child| DOTS.contains(&context.source.node_text(*child)))
}
