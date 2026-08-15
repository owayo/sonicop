use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

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
        let Some(selector) = node.field("method") else {
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
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| !child.is_named() && child.start_byte() >= receiver.end_byte())
        .find(|child| DOTS.contains(&context.source.node_text(*child)))
}
