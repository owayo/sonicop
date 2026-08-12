//! The `PercentLiteral` mixin: which percent literal a node is, and what it was written from.
//!
//! Upstream reaches these cops through `on_array` and asks the literal's opening delimiter what
//! kind it is -- `%w[` answers `%w`. tree-sitter names the two array kinds after their contents
//! rather than their delimiters, so the prefix is read the same way but from a node that already
//! says which of the two it is.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::send_node::{has_interpolation, named_children};

/// `type(node)`: the opening delimiter without its bracket, or `None` when the literal was not
/// written as a percent literal at all.
pub(super) fn percent_type<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let opening = node.child(0)?;
    let text = context.source.node_text(opening);
    text.starts_with('%').then(|| &text[..text.len() - 1])
}

/// The elements of a percent literal, which are its named children.
pub(super) fn values<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(node)
        .into_iter()
        .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
        .collect()
}

/// `child.children.first.to_s`: the text one element stands for. An element that interpolates has
/// a node rather than a string there, which no pattern this mixin's cops apply can match.
pub(super) fn value_text<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    (!has_interpolation(node)).then(|| context.source.node_text(node))
}
