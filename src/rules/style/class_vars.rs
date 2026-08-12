//! `Style/ClassVars`: a class variable is shared with every subclass, so assigning one reaches
//! further than the class that wrote it.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_cvasgn`: only assignment is reported, so a class variable merely read is left alone.
    for node in context.nodes_of("class_variable") {
        if !is_assignment_target(node) {
            continue;
        }
        offenses.push(context.offense(message(context.source.node_text(node)), node.byte_range()));
    }
    // `RESTRICT_ON_SEND = %i[class_variable_set]`: the reflective spelling of the same assignment.
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(method) != "class_variable_set" {
            continue;
        }
        let Some(first) = arguments(node).into_iter().next() else {
            continue;
        };
        let range = first.range();
        offenses.push(context.offense(message(context.source.slice(range.clone())), range));
    }
}

fn message(class_var: &str) -> String {
    format!("Replace class var {class_var} with a class instance var.")
}

/// Whether the variable stands where upstream's parser builds a `cvasgn` rather than a `cvar`.
fn is_assignment_target(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        // `@@a = 1`, `@@a += 1`, `@@a ||= 1`.
        "assignment" | "operator_assignment" => parent
            .child_by_field_name("left")
            .is_some_and(|left| left.id() == node.id()),
        // `@@a, @@b = 1, 2` and the `*@@rest` of it.
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => true,
        // `for @@a in list`.
        "for" => parent
            .child_by_field_name("pattern")
            .is_some_and(|pattern| pattern.id() == node.id()),
        // `rescue => @@error`.
        "exception_variable" => true,
        // `each { |@@a| }`: a block parameter binds the variable just as an assignment does.
        "block_parameters" => true,
        _ => false,
    }
}
