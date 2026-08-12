use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Omit parentheses for the empty lambda parameters.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("lambda") {
        let Some(parameters) = node.child_by_field_name("parameters") else {
            continue;
        };
        // `empty_and_without_delimiters?`: parentheses were written around nothing.
        if !super::nodes::children(parameters).is_empty() {
            continue;
        }
        // `send_node.source_range.end_pos`: the end of the `->` the parameters follow.
        let Some(arrow) = node.child(0) else {
            continue;
        };
        offenses.push(
            context
                .offense(MSG, parameters.byte_range())
                .corrected_by(Edit {
                    start: arrow.end_byte(),
                    end: parameters.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
