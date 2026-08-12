use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Place the end statement of a multi-line method on its own line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(end) = node
            .child(node.child_count().saturating_sub(1) as u32)
            .filter(|last| last.kind() == "end")
        else {
            continue;
        };
        // `trailing_end?`: a body that runs up to the line the `end` is on.
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        let Some(last) = super::nodes::children(body).last().copied() else {
            continue;
        };
        if node.start_position().row == node.end_position().row
            || last.end_position().row != end.end_position().row
        {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, end.byte_range())
                .corrected_by(Edit {
                    start: end.start_byte(),
                    end: end.start_byte(),
                    replacement: format!("\n{}", " ".repeat(node.start_position().column)),
                    safe: true,
                }),
        );
    }
}
