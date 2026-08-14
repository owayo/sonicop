use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const MSG: &str = "Avoid chaining a method call on a do...end block.";

/// `"end".len()`, which is what the `end` keyword of a `do` block always spans.
const END_LENGTH: usize = 3;

/// A call whose receiver is a `do`...`end` block.
///
/// Upstream reaches the same shape through `receiver.any_block_type? && receiver.keywords?`, where
/// the receiver is the `block` node wrapped around the inner call. Here the block is a field of the
/// call itself, so the test is whether the receiver carries a `do_block`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let Some(block) = do_block_of(receiver) else {
            continue;
        };
        // `range_between(receiver.loc.end.begin_pos, node.source_range.end_pos)`: from the `end`
        // keyword through the end of the call.
        let start = block.end_byte() - END_LENGTH;
        offenses.push(context.offense(MSG, start..send_node::send_range(node, context).end));
    }
}

/// The `do`...`end` block written on the node, if that is the kind of block it has.
fn do_block_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let block = match node.kind_str() {
        // `-> do ... end` is a `block` node upstream as well.
        "lambda" => node.field("body")?,
        "call" => node.field("block")?,
        _ => return None,
    };
    (block.kind_str() == "do_block").then_some(block)
}
