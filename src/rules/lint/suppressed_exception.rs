use super::rescue_clause::{body, end};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use tree_sitter::Node;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not suppress exceptions.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    let allow_nil: bool = context.setting("AllowNil").unwrap_or(true);
    // `on_resbody`: the modifier form is a `resbody` too -- `something rescue nil` suppresses just
    // as silently as the block form.
    for node in context.nodes_of_any(&["rescue", "rescue_modifier"]) {
        let statements = match node.kind_str() {
            "rescue_modifier" => node.field("handler").into_iter().collect(),
            _ => body(node),
        };
        let empty = statements.is_empty();
        let nil_body = statements.len() == 1 && statements[0].kind_str() == "nil";
        if !(empty || nil_body)
            || (allow_comments && commented(node, context))
            || (allow_nil && nil_body)
        {
            continue;
        }
        let range = match node.kind_str() {
            // The offense covers the `rescue` keyword and what follows it, not the guarded body.
            "rescue_modifier" => {
                let keyword = (0..node.child_count())
                    .filter_map(|index| node.child(index as u32))
                    .find(|child| context.source.node_text(*child) == "rescue");
                match keyword {
                    Some(keyword) => keyword.start_byte()..node.end_byte(),
                    None => node.byte_range(),
                }
            }
            _ => node.start_byte()..end(node, &statements),
        };
        offenses.push(context.offense(MSG, range));
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
            ancestor.kind_str(),
            "begin" | "method" | "singleton_method" | "block" | "do_block"
        ) {
            return Some(ancestor);
        }
        current = ancestor.parent();
    }
    None
}
