//! Tree walks shared by cops in more than one department.

use tree_sitter::Node;

/// Pushes `node`'s named children so that popping the stack yields them in
/// source order, making a `pop`-driven loop reproduce depth-first pre-order.
pub(crate) fn push_named_children<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let start = stack.len();
    let mut cursor = node.walk();
    stack.extend(node.named_children(&mut cursor));
    stack[start..].reverse();
}

pub(crate) fn walk_named(node: Node<'_>, callback: &mut impl FnMut(Node<'_>)) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        callback(current);
        push_named_children(current, &mut stack);
    }
}

pub(crate) fn first_identifier(node: Node<'_>) -> Option<Node<'_>> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            return Some(current);
        }
        push_named_children(current, &mut stack);
    }
    None
}
