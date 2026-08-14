use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not define methods at the top-level.";

/// A `def`, a `def self.`, or a `define_method` written where nothing encloses it.
///
/// `top_level_method_definition?` asks whether the node is the root or sits directly in the root's
/// statement list, which is the same as having `program` for a parent here.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method", "call"]) {
        if !node
            .parent()
            .is_some_and(|parent| parent.kind_str() == "program")
        {
            continue;
        }
        if node.kind_str() == "call" && !defines_a_method(node, context) {
            continue;
        }
        // A `define_method` carrying a block is one `block` node upstream, so the block belongs
        // inside the report.
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// `define_method`, either bare (`on_send`) or with a block (`(any_block (send _ :define_method _)
/// ...)`, which insists on exactly one argument).
fn defines_a_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node
        .field("method")
        .is_none_or(|selector| context.source.node_text(selector) != "define_method")
    {
        return false;
    }
    if node.field("block").is_none() {
        return true;
    }
    node.field("arguments")
        .is_some_and(|arguments| super::nodes::children(arguments).len() == 1)
}
