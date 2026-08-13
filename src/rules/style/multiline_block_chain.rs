use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::send_range;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if call.child_by_field_name("block").is_none() {
            continue;
        }
        let send = send_range(call, context);
        // `node.send_node.each_node(:call)`, which stops at the first chained block it finds.
        let Some(closing) = super::conditional::descendants(call)
            .into_iter()
            .filter(|node| node.start_byte() < send.end)
            .find_map(|node| chained_block_end(node, context))
        else {
            continue;
        };
        offenses.push(context.offense(
            "Avoid multi-line chains of blocks.",
            closing.start_byte()..send.end,
        ));
    }
}

/// The `end` or `}` of a multiline block standing where the receiver of `node` was written, which
/// is what `receiver&.any_block_type? && receiver.multiline?` asks for.
fn chained_block_end<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let receiver = receiver_of(node, context)?;
    if receiver.start_position().row == receiver.end_position().row {
        return None;
    }
    // `-> { }` is a block upstream just as `foo { }` is, and the grammar spells the two apart.
    let block = match receiver.kind() {
        "call" => receiver.child_by_field_name("block")?,
        "lambda" => receiver.child_by_field_name("body")?,
        _ => return None,
    };
    block.child(block.child_count().checked_sub(1)? as u32)
}

/// The receiver of the node, for the shapes the grammar writes a call with one in. `a && b` is an
/// `and` node upstream rather than a call, so only the operators a class may redefine count.
fn receiver_of<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    match node.kind() {
        "call" => node.child_by_field_name("receiver"),
        "element_reference" => node.child_by_field_name("object"),
        "unary" => node.child_by_field_name("operand"),
        "binary" => {
            let operator = context
                .source
                .node_text(node.child_by_field_name("operator")?);
            super::nodes::is_operator_method(operator)
                .then(|| node.child_by_field_name("left"))
                .flatten()
        }
        _ => None,
    }
}
