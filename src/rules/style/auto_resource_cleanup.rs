use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::is_plain_send;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `(send (const {nil? cbase} {:File :Tempfile}) :open ...)`: an opened resource with nobody to
/// close it.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method")) else {
            continue;
        };
        if context.source.node_text(selector) != "open" {
            continue;
        }
        if !send_node::top_level_constant(receiver, "File", context)
            && !send_node::top_level_constant(receiver, "Tempfile", context)
        {
            continue;
        }
        if cleans_up(node, context) {
            continue;
        }
        let current = &context.source.text()[receiver.start_byte()..selector.end_byte()];
        offenses.push(context.offense(
            format!("Use the block version of `{current}`."),
            send_node::send_range(node, context),
        ));
    }
}

/// `cleanup?`.
///
/// A block or a `&block` pass takes care of closing, and so does anything that is not an assignment
/// to a local variable -- upstream only reports the handle that gets kept.
fn cleans_up(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.field("block").is_some() {
        return true;
    }
    let arguments = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    if arguments
        .iter()
        .any(|argument| argument.kind_str() == "block_argument")
    {
        return true;
    }
    // `return false unless (parent = node.parent)`: upstream's parent is nil when the call is the
    // whole file, which here is a `program` holding nothing else.
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind_str() == "program" {
        let statements: Vec<Node<'_>> = super::nodes::children(parent)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect();
        if statements.len() == 1 {
            return false;
        }
        return true;
    }
    // `parent.block_type? || !parent.lvasgn_type?`.
    let assigns_to_local = parent.kind_str() == "assignment"
        && parent
            .field("left")
            .is_some_and(|target| target.kind_str() == "identifier")
        && parent
            .field("right")
            .is_some_and(|value| value.id() == node.id());
    let _ = context;
    !assigns_to_local
}
