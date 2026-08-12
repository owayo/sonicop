use super::rescue_clause::{body, end};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use tree_sitter::Node;

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
        offenses.push(context.offense(MSG, node.start_byte()..end(node, &statements)));
    }
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
