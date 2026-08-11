use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::{RuleContext, first_identifier, walk_named};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_empty: bool = context.setting("IgnoreEmptyBlocks").unwrap_or(true);
    for node in context.nodes_of_any(&["block", "do_block"]) {
        let (Some(parameters), Some(body)) = (
            node.child_by_field_name("parameters"),
            node.child_by_field_name("body"),
        ) else {
            continue;
        };
        if ignore_empty && context.source.node_text(body).trim().is_empty() {
            continue;
        }

        let mut parameter_nodes = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            if parameter.kind() == "identifier" {
                if seen.insert(parameter.start_byte()) {
                    parameter_nodes.push(parameter);
                }
            } else if let Some(identifier) = first_identifier(parameter)
                && seen.insert(identifier.start_byte())
            {
                parameter_nodes.push(identifier);
            }
        }

        for parameter in parameter_nodes {
            let name = context.source.node_text(parameter);
            if name.starts_with('_') || identifier_used(body, name, parameter, context) {
                continue;
            }
            offenses.push(
                context
                    .offense(
                        format!("Unused block argument - `{name}`. If it's necessary, use `_` or `_name` as an argument name."),
                        parameter.byte_range(),
                    )
                    .corrected_by(Edit {
                        start: parameter.start_byte(),
                        end: parameter.start_byte(),
                        replacement: "_".to_owned(),
                        safe: true,
                    }),
            );
        }
    }
}

fn identifier_used(
    body: Node<'_>,
    name: &str,
    definition: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let mut used = false;
    walk_named(body, &mut |candidate| {
        if candidate.kind() == "identifier"
            && candidate.byte_range() != definition.byte_range()
            && context.source.node_text(candidate) == name
        {
            used = true;
        }
    });
    used
}
