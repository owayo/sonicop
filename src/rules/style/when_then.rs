//! `Style/WhenThen`: a one-line `when` separates its body with `then`, not with a semicolon.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("when") {
        if node.start_position().row != node.end_position().row {
            continue;
        }
        let Some(body) = node.field("body") else {
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
        if super::nodes::children_in(body, context).is_empty() {
            continue;
        }
        let conditions: Vec<&str> = named_children_of(node, context)
            .into_iter()
            .filter(|child| child.kind_str() == "pattern")
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
