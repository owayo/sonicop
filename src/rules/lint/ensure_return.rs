use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::statements::statements;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not return from an `ensure` block.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("ensure") {
        for statement in statements(node) {
            collect(statement, node, context, offenses);
        }
    }
}

/// `node.branch.each_node(:return)`, which starts at the branch itself rather than below it.
fn collect(
    node: Node<'_>,
    ensure_node: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    if node.kind_str() == "return" && !returns_from_inner_scope(node, ensure_node, context) {
        offenses.push(context.offense(MSG, node.byte_range()));
    }
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
    for child in children {
        collect(child, ensure_node, context, offenses);
    }
}

/// Whether the `return` leaves an inner scope rather than the method the `ensure` belongs to. A
/// method definition and a lambda both return to their own caller; a plain block does not, so a
/// `return` written in one still preempts the exception.
fn returns_from_inner_scope(
    node: Node<'_>,
    ensure_node: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if ancestor.id() == ensure_node.id() {
            return false;
        }
        if matches!(ancestor.kind_str(), "method" | "singleton_method" | "lambda")
            || is_lambda_block(ancestor, context)
        {
            return true;
        }
        current = ancestor.parent_of(context);
    }
    false
}

/// `any_block_type? && lambda?`: a block written as `lambda { ... }`. `proc { ... }` is not one, and
/// neither is any other method taking a block.
fn is_lambda_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(node.kind_str(), "block" | "do_block")
        && node.parent_of(context).is_some_and(|call| {
            call.kind_str() == "call"
                && call
                    .field("method")
                    .is_some_and(|method| context.source.node_text(method) == "lambda")
        })
}
