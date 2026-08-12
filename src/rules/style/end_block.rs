use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Avoid the use of `END` blocks. Use `Kernel#at_exit` instead.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("end_block") {
        let Some(keyword) = node.child(0) else {
            continue;
        };
        offenses.push(
            context
                .offense(MSG, keyword.byte_range())
                .corrected_by(Edit {
                    start: keyword.start_byte(),
                    end: keyword.end_byte(),
                    replacement: "at_exit".to_owned(),
                    safe: true,
                }),
        );
    }
}
