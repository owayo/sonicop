use std::collections::HashSet;

use tree_sitter::Node;

use super::support::valid_name;
use crate::diagnostic::Offense;
use crate::rules::{RuleContext, first_identifier};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "snake_case".to_owned());
    let mut seen = HashSet::new();
    for node in context.nodes_of("identifier") {
        if !is_variable_definition(node) || !seen.insert(node.start_byte()) {
            continue;
        }
        let name = context.source.node_text(node);
        if valid_name(name, &style) {
            continue;
        }
        offenses.push(context.offense(
            format!("Use {style} for variable names."),
            node.byte_range(),
        ));
    }
}

/// Whether the identifier introduces a variable rather than reading one.
fn is_variable_definition(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        "assignment" | "operator_assignment" => parent
            .child_by_field_name("left")
            .is_some_and(|left| left.byte_range() == node.byte_range()),
        "method_parameters" | "block_parameters" | "lambda_parameters" => true,
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter"
        | "destructured_parameter"
        | "rescue" => {
            first_identifier(parent).is_some_and(|first| first.byte_range() == node.byte_range())
        }
        _ => false,
    }
}
