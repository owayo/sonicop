use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, send_range};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "The argument to each_with_object cannot be immutable.";

/// `IMMUTABLE_LITERALS`: `LITERALS - MUTABLE_LITERALS`. Everything left is a value the block cannot
/// accumulate into, so the call can only return what it was handed.
const IMMUTABLE: &[&str] = &[
    "integer",
    "float",
    "rational",
    "complex",
    "true",
    "false",
    "nil",
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    "bare_symbol",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "each_with_object"
            || node.field("receiver").is_none()
        {
            continue;
        }
        let arguments = arguments(node);
        let [argument] = arguments.as_slice() else {
            continue;
        };
        if !immutable(argument.first(), context) {
            continue;
        }
        offenses.push(context.offense(MSG, send_range(node, context)));
    }
}

/// `Node#immutable_literal?`. The parser folds a leading sign into the number it precedes, so
/// `-1` is one `int` here as well.
fn immutable(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "unary" => {
            node.field("operator")
                .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
                && node
                    .field("operand")
                    .is_some_and(|operand| matches!(operand.kind_str(), "integer" | "float"))
        }
        kind => IMMUTABLE.contains(&kind),
    }
}
