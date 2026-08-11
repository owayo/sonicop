//! Reading a tree-sitter node the way upstream's parser would.

use tree_sitter::Node;

/// Node kinds tree-sitter parks in the tree that upstream's AST has no child for.
///
/// A heredoc's body is spelled as a sibling of the statement that opened it, so a literal holding
/// one would otherwise report an extra element after its last real one.
const NOT_A_CHILD: &[&str] = &["comment", "heredoc_body"];

pub(super) fn is_child(node: Node<'_>) -> bool {
    !NOT_A_CHILD.contains(&node.kind())
}

/// The node's children as upstream's AST holds them.
pub(super) fn children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| is_child(*child))
        .collect()
}
