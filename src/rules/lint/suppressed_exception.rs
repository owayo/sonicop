use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Do not suppress exceptions.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    let allow_nil: bool = context.setting("AllowNil").unwrap_or(true);
    for node in context.nodes_of("rescue") {
        let statements = body(node);
        let empty = statements.is_empty();
        let nil_body = statements.len() == 1 && statements[0].kind() == "nil";
        if !(empty || nil_body)
            || (allow_comments && commented(node, context))
            || (allow_nil && nil_body)
        {
            continue;
        }
        offenses.push(context.offense(MSG, node.start_byte()..clause_end(node, &statements)));
    }
}

/// Where the clause really ends. The grammar lets a `rescue` node run on over the trailing
/// comments and the `;` that separates it from what comes next, which upstream's node stops short
/// of -- but the `;` or `then` that introduces the body *is* part of it, empty body or not.
fn clause_end(node: Node<'_>, statements: &[Node<'_>]) -> usize {
    let mut end = node.start_byte();
    let parts = ["exceptions", "variable"]
        .iter()
        .filter_map(|field| node.child_by_field_name(field));
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
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        tokens.extend(body.children(&mut body_cursor));
    }
    for token in tokens {
        if !token.is_named()
            && matches!(token.kind(), ";" | "then")
            && token.start_byte() < first_statement
        {
            end = end.max(token.end_byte());
        }
    }
    end
}

/// The statements the clause handles the exception with. A `;` is not one of them, and neither is
/// a comment -- which is exactly why `AllowComments` has to look at the source lines instead.
fn body<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "empty_statement" | "comment" | "heredoc_body"))
        .collect()
}

/// Whether a comment stands between the `rescue` and the `end` that closes what it belongs to.
/// A clause that says in words why the exception is ignored is not suppressing it silently.
fn commented(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(ancestor) = enclosing_body(node) else {
        return false;
    };
    let first = node.start_position().row + 1;
    let last = ancestor.end_position().row + 1;
    (first + 1..=last).any(|line| context.source.line(line).trim_start().starts_with('#'))
}

/// The `begin`, method or block the clause belongs to, whose `end` bounds the lines to search. A
/// `rescue` written anywhere else -- in a class body, say -- has no such bound and is not excused.
fn enclosing_body(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if matches!(
            ancestor.kind(),
            "begin" | "method" | "singleton_method" | "block" | "do_block"
        ) {
            return Some(ancestor);
        }
        current = ancestor.parent();
    }
    None
}
