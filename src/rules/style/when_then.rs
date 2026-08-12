//! `Style/WhenThen`: a one-line `when` separates its body with `then`, not with a semicolon.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("when") {
        if node.start_position().row != node.end_position().row {
            continue;
        }
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        // `node.then?`: the separator is already the keyword.
        let Some(separator) = body.child(0).filter(|first| !first.is_named()) else {
            continue;
        };
        if context.source.node_text(separator) != ";" {
            continue;
        }
        // `!node.body`: `when 1; end` writes the separator but no statement after it.
        if super::nodes::children(body).is_empty() {
            continue;
        }
        let mut cursor = node.walk();
        let conditions: Vec<&str> = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "pattern")
            .map(|pattern| context.source.node_text(pattern))
            .collect();
        let expression = conditions.join(", ");
        let message =
            format!("Do not use `when {expression};`. Use `when {expression} then` instead.");
        offenses.push(
            context
                .offense(message, separator.byte_range())
                .corrected_by(Edit {
                    start: separator.start_byte(),
                    end: separator.end_byte(),
                    replacement: " then".to_owned(),
                    safe: true,
                }),
        );
    }
}
