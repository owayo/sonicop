use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;

const MSG: &str = "Use the double pipe equals operator `||=` instead.";

/// The left-hand sides the parser spells as a plain variable assignment, and the reads that match
/// them.
const VARIABLES: &[&str] = &[
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_lvasgn`: `name = name ? name : 'x'`.
    for node in context.nodes_of("assignment") {
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            continue;
        };
        if !VARIABLES.contains(&left.kind()) || right.kind() != "conditional" {
            continue;
        }
        let name = context.source.node_text(left);
        let (Some(condition), Some(consequence), Some(alternative)) = (
            right.child_by_field_name("condition"),
            right.child_by_field_name("consequence"),
            right.child_by_field_name("alternative"),
        ) else {
            continue;
        };
        if !reads(context, condition, left, name) || !reads(context, consequence, left, name) {
            continue;
        }
        // `return if else_branch.if_type?`.
        if matches!(
            alternative.kind(),
            "if" | "unless" | "conditional" | "if_modifier" | "unless_modifier"
        ) {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!("{name} ||= {}", context.source.node_text(alternative)),
                    safe: true,
                }),
        );
    }

    // `on_if`: `name = 'x' unless name`, in either form.
    let mut locals = None;
    for node in context.nodes_of_any(&["unless", "unless_modifier"]) {
        if node.child_by_field_name("alternative").is_some() {
            continue;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        if !VARIABLES.contains(&condition.kind()) {
            continue;
        }
        // `{lvar ivar cvar gvar}`: a bare name is only one of those once it has been assigned, and
        // a first mention is a receiverless call. The modifier form assigns before the condition is
        // even read, so only the keyword form has to ask.
        if node.kind() == "unless"
            && condition.kind() == "identifier"
            && !locals
                .get_or_insert_with(|| LocalVariables::new(context))
                .is_lvar(condition)
        {
            continue;
        }
        let Some(body) = body_statement(node) else {
            continue;
        };
        if body.kind() != "assignment" {
            continue;
        }
        let (Some(left), Some(right)) = (
            body.child_by_field_name("left"),
            body.child_by_field_name("right"),
        ) else {
            continue;
        };
        if left.kind() != condition.kind()
            || context.source.node_text(left) != context.source.node_text(condition)
        {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!(
                        "{} ||= {}",
                        context.source.node_text(left),
                        context.source.node_text(right)
                    ),
                    safe: true,
                }),
        );
    }
}

/// `({lvar ivar cvar gvar} _var)`: a read of the very variable being assigned.
fn reads(context: &RuleContext<'_>, node: Node<'_>, left: Node<'_>, name: &str) -> bool {
    node.kind() == left.kind() && context.source.node_text(node) == name
}

/// The one statement the `unless` guards.
fn body_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    // The keyword form spells its body as a `then` clause; the modifier form has it directly.
    let body = node
        .child_by_field_name("body")
        .or_else(|| node.child_by_field_name("consequence"))?;
    match body.kind() {
        "then" => match super::nodes::children(body).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        _ => Some(body),
    }
}
