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

/// Whether two nodes are the same node, which is what comparing two AST nodes asks upstream.
///
/// Three details of the grammar have to be put back for the answer to be upstream's: an operator
/// is an anonymous child here and the *name* of the call upstream, a heredoc's body is written as
/// a sibling of the statement that opened it rather than inside the literal, and a comment is a
/// child of the statement list it was written in.
pub(super) fn same_tree(context: &RuleContext<'_>, left: Node<'_>, right: Node<'_>) -> bool {
    if left.kind() != right.kind() {
        return false;
    }
    let operator = |node: Node<'_>| {
        node.child_by_field_name("operator")
            .map(|operator| context.source.node_text(operator))
    };
    if operator(left) != operator(right) {
        return false;
    }
    if left.kind() == "heredoc_beginning" {
        return heredoc_text(context, left) == heredoc_text(context, right);
    }
    let (left_children, right_children) = (children(left), children(right));
    if left_children.is_empty() && right_children.is_empty() {
        return context.source.node_text(left) == context.source.node_text(right);
    }
    left_children.len() == right_children.len()
        && left_children
            .iter()
            .zip(&right_children)
            .all(|(left, right)| same_tree(context, *left, *right))
}

/// The text of the heredoc a `heredoc_beginning` opened, which the grammar parks after the
/// statement rather than inside the literal.
fn heredoc_text<'a>(context: &'a RuleContext<'_>, beginning: Node<'_>) -> Option<&'a str> {
    let body = crate::rules::send_node::heredoc_body(beginning, context)?;
    Some(context.source.node_text(body))
}

/// Whether an `assignment` node is the grammar's misreading of a `=~` match.
///
/// `a[0] =~ /x/` parses as an assignment of `~ /x/` to `a[0]`, because an indexing is a valid
/// assignment target and the grammar prefers that reading. Ruby lexes `=~` as one operator, so a
/// `=` written straight against a `~` is never an assignment.
pub(super) fn is_match_assignment(node: Node<'_>, text: &str) -> bool {
    let Some(left) = node.child_by_field_name("left") else {
        return false;
    };
    let Some(operator) = left.next_sibling() else {
        return false;
    };
    &text[operator.byte_range()] == "=" && text.as_bytes().get(operator.end_byte()) == Some(&b'~')
}
