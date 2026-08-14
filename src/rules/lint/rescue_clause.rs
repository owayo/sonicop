//! What a `rescue` clause covers, shared by the cops that report one.
//!
//! The grammar lets a `rescue` node run on over the trailing comments and the `;` that separates
//! it from what comes next, which upstream's `resbody` stops short of -- but the `;` or `then`
//! that introduces the body *is* part of it, empty body or not.

use crate::rules::node_ext::NodeExt;
use tree_sitter::Node;

/// The statements the clause handles the exception with. A `;` is not one of them, and neither is
/// a comment or the body of a heredoc opened in it.
pub(super) fn body<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = node.field("body") else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind_str(),
                "empty_statement" | "comment" | "heredoc_body"
            )
        })
        .collect()
}

/// Where the clause ends, as upstream's node ends.
pub(super) fn end(node: Node<'_>, statements: &[Node<'_>]) -> usize {
    let mut end = node.start_byte();
    let parts = ["exceptions", "variable"]
        .iter()
        .filter_map(|field| node.field(field));
    for part in node
        .child(0)
        .into_iter()
        .chain(parts)
        .chain(statements.iter().copied())
    {
        end = end.max(part.end_byte());
    }
    let first_statement = statements
        .first()
        .map_or(usize::MAX, |statement| statement.start_byte());
    let mut cursor = node.walk();
    let mut tokens: Vec<Node<'_>> = node.children(&mut cursor).collect();
    if let Some(body) = node.field("body") {
        let mut body_cursor = body.walk();
        tokens.extend(body.children(&mut body_cursor));
    }
    for token in tokens {
        if !token.is_named()
            && matches!(token.kind_str(), ";" | "then")
            && token.start_byte() < first_statement
        {
            end = end.max(token.end_byte());
        }
    }
    end
}

/// `Node#const_name`. A leading `::` names the same constant, so `::Exception` reads as
/// `Exception`, while a namespace that is not itself a constant contributes nothing.
pub(super) fn const_name(node: Node<'_>, source: &crate::source::SourceFile) -> Option<String> {
    let name = match node.kind_str() {
        "constant" => return Some(source.node_text(node).to_owned()),
        "scope_resolution" => source.node_text(node.field("name")?),
        _ => return None,
    };
    match node.field("scope") {
        Some(scope) => Some(format!(
            "{}::{name}",
            const_name(scope, source).unwrap_or_default()
        )),
        None => Some(name.to_owned()),
    }
}
