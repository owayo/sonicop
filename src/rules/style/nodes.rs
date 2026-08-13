//! Reading a tree-sitter node the way upstream's parser would.

use tree_sitter::Node;

use crate::rules::RuleContext;

/// Node kinds tree-sitter parks in the tree that upstream's AST has no child for.
///
/// A heredoc's body is spelled as a sibling of the statement that opened it, so a literal holding
/// one would otherwise report an extra element after its last real one.
const NOT_A_CHILD: &[&str] = &["comment", "heredoc_body"];

pub(super) fn is_child(node: Node<'_>) -> bool {
    !NOT_A_CHILD.contains(&node.kind())
}

/// Node kinds the grammar leaves as the left operand of a binary expression where upstream's
/// parser reads a jump with an argument instead.
///
/// `return +""` is `(return (send (str "") :+@))` upstream: a bare keyword cannot be an operand of
/// anything, so a binary expression that seems to have one is the grammar's reading alone.
const BARE_JUMP: &[&str] = &["return", "break", "next", "redo", "retry", "yield"];

pub(super) fn is_bare_jump(node: Node<'_>) -> bool {
    BARE_JUMP.contains(&node.kind())
}

/// The operators a class may redefine, which are the ones upstream's parser spells as a `send`.
/// `&&`, `||`, `and` and `or` are not among them: those are `and` / `or` nodes there.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

pub(super) fn is_operator_method(name: &str) -> bool {
    OPERATOR_METHODS.contains(&name)
}

/// The node's children as upstream's AST holds them.
pub(super) fn children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| is_child(*child))
        .collect()
}

/// `ProcessedSource#contains_comment?`: whether a comment sits on any line the range spans. It
/// reads *lines* rather than the range itself, so a trailing comment on the closing line counts.
pub(super) fn contains_comment(range: &std::ops::Range<usize>, context: &RuleContext<'_>) -> bool {
    let first = context.source.line_column(range.start).0;
    let last = context.source.line_column(range.end).0;
    context.comment_ranges().iter().any(|comment| {
        let line = context.source.line_column(comment.start).0;
        (first..=last).contains(&line)
    })
}
