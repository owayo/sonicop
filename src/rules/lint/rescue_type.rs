use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::literals::literal_type;

/// `INVALID_TYPES`: the literals that name a value rather than a class, so that `rescue` raises a
/// `TypeError` when it tries to match against them.
const INVALID_TYPES: &[&str] = &[
    "array", "complex", "dstr", "false", "float", "hash", "nil", "int", "rational", "str", "sym",
    "true",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for clause in context.nodes_of("rescue") {
        let Some(keyword) = clause.child(0) else {
            continue;
        };
        let Some(list) = clause.child_by_field_name("exceptions") else {
            continue;
        };
        let mut cursor = list.walk();
        let exceptions: Vec<Node<'_>> = list.named_children(&mut cursor).collect();
        let invalid: Vec<&Node<'_>> = exceptions
            .iter()
            .filter(|exception| is_invalid(**exception, context))
            .collect();
        if invalid.is_empty() {
            continue;
        }
        let sources: Vec<&str> = invalid
            .iter()
            .map(|exception| context.source.node_text(**exception))
            .collect();
        // `valid_exceptions.map(&:source).join(', ')`, with a leading space unless it is empty.
        let mut replacement = exceptions
            .iter()
            .filter(|exception| !is_invalid(**exception, context))
            .map(|exception| context.source.node_text(*exception))
            .collect::<Vec<&str>>()
            .join(", ");
        if !replacement.is_empty() {
            replacement.insert(0, ' ');
        }
        offenses.push(
            context
                .offense(
                    format!(
                        "Rescuing from `{}` will raise a `TypeError` instead of catching the actual exception.",
                        sources.join(", ")
                    ),
                    keyword.start_byte()..list.end_byte(),
                )
                .corrected_by(Edit {
                    start: keyword.end_byte(),
                    end: list.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

fn is_invalid(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    literal_type(node, context).is_some_and(|kind| INVALID_TYPES.contains(&kind))
}
