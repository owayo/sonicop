use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

/// The node kinds a top-level statement may be wrapped in and still be top level: upstream's
/// `{kwbegin begin if def}`, plus the grammar's own statement containers, which have no counterpart
/// in the parser's tree at all.
const TRANSPARENT: &[&str] = &[
    "begin",
    "program",
    "then",
    "else",
    "elsif",
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "method",
    "body_statement",
    "parenthesized_statements",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node.child_by_field_name("receiver").is_some()
            || node.child_by_field_name("block").is_some()
        {
            continue;
        }
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        let statement = context.source.node_text(method);
        if !matches!(statement, "include" | "extend" | "prepend") {
            continue;
        }
        // `const+`: every argument has to be a constant, and there has to be one.
        let arguments = node
            .child_by_field_name("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        if arguments.is_empty()
            || !arguments
                .iter()
                .all(|argument| matches!(argument.kind(), "constant" | "scope_resolution"))
        {
            continue;
        }
        if !in_top_level_scope(node) {
            continue;
        }
        offenses.push(context.offense(
            format!("`{statement}` is used at the top level. Use inside `class` or `module`."),
            node.byte_range(),
        ));
    }
}

/// `in_top_level_scope?`: nothing but a `begin`, a conditional or a method definition may stand
/// between the call and the root.
fn in_top_level_scope(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), |current| current.parent())
        .all(|ancestor| TRANSPARENT.contains(&ancestor.kind()))
}
