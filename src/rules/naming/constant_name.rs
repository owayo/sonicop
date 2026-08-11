use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

static CONSTANT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]*$").unwrap());

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("assignment") {
        let Some(left) = node.child_by_field_name("left") else {
            continue;
        };
        if left.kind() != "constant" {
            continue;
        }
        let name = context.source.node_text(left);
        // Only a literal makes the name a constant in RuboCop's sense; anything computed may well
        // be a class or module the author named in CamelCase on purpose.
        let allowed_assignment = node
            .child_by_field_name("right")
            .is_none_or(|right| !literal_constant_value(right));
        if allowed_assignment || CONSTANT.is_match(name) {
            continue;
        }
        offenses
            .push(context.offense("Use SCREAMING_SNAKE_CASE for constants.", left.byte_range()));
    }
}

fn literal_constant_value(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "integer"
            | "float"
            | "rational"
            | "complex"
            | "string"
            | "symbol"
            | "array"
            | "hash"
            | "true"
            | "false"
            | "nil"
            | "regex"
    )
}
