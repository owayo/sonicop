use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Do not define constants this way within a block.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    for node in context.nodes_of_any(&["assignment", "class", "module"]) {
        // Only a constant assigned by its bare name is reported: `Foo::BAR = 1` names where it
        // goes, so the block it was written in does not decide that.
        if node.kind() == "assignment"
            && !node
                .child_by_field_name("left")
                .is_some_and(|left| left.kind() == "constant")
        {
            continue;
        }
        let Some(block) = enclosing_block(node) else {
            continue;
        };
        if allowed
            .iter()
            .any(|method| method == block_method(block, context))
        {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// The block whose body the definition is a statement of. Upstream asks whether the parent is a
/// block, or a `begin` inside one -- which is the same as being a statement written directly in
/// the block, since a body of several statements is what makes the `begin`.
fn enclosing_block(node: Node<'_>) -> Option<Node<'_>> {
    let body = node.parent()?;
    if !matches!(body.kind(), "block_body" | "body_statement") {
        return None;
    }
    // A `rescue`, `else` or `ensure` clause makes the body a node of its own upstream, with the
    // statements one level further down again -- so nothing in such a block is written directly in
    // it any more.
    let mut cursor = body.walk();
    if body
        .named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "rescue" | "else" | "ensure"))
    {
        return None;
    }
    body.parent()
        .filter(|block| matches!(block.kind(), "block" | "do_block"))
}

fn block_method<'a>(block: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    block
        .parent()
        .filter(|parent| parent.kind() == "call")
        .and_then(|call| call.child_by_field_name("method"))
        .map_or("", |method| context.source.node_text(method))
}
