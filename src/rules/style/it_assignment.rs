//! `Style/ItAssignment`: `it` is what a block's sole parameter is called by default.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "`it` is the default block parameter; consider another name.";

/// The parameter kinds that name a local variable, which is what the `on_arg` family covers.
const NAMED_PARAMETERS: &[&str] = &[
    "optional_parameter",
    "splat_parameter",
    "hash_splat_parameter",
    "block_parameter",
    "keyword_parameter",
];

/// The lists whose bare identifiers are parameters rather than calls.
const PARAMETER_LISTS: &[&str] = &["method_parameters", "block_parameters", "lambda_parameters"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("identifier") {
        if context.source.node_text(node) != "it" || !is_binding(node, context) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// Whether the identifier is the name upstream's parser writes as an `lvasgn` or an `arg`.
fn is_binding(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = context.parent(node) else {
        return false;
    };
    match parent.kind_str() {
        // `it = 1`, `it ||= 1` and each target of `it, x = 1, 2`.
        "assignment" | "operator_assignment" | "left_assignment_list" => parent
            .field("left")
            .is_none_or(|left| left.id() == node.id()),
        // `def m(it)`, `foo { |it| }`, `->(it) {}`.
        kind if PARAMETER_LISTS.contains(&kind) => true,
        // `def m(it = 1)` and the four sigils.
        kind if NAMED_PARAMETERS.contains(&kind) => parent
            .field("name")
            .is_some_and(|name| name.id() == node.id()),
        // `for it in ...`.
        "for" => parent
            .field("pattern")
            .is_some_and(|pattern| pattern.id() == node.id()),
        // `rescue => it`.
        "exception_variable" => true,
        _ => false,
    }
}
