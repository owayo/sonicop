use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Do not use `::` for defining class methods.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("singleton_method") {
        // `node.loc.operator` is the token between the singleton and the method name, which the
        // grammar leaves unnamed.
        let Some(operator) = node
            .children(&mut node.walk())
            .find(|child| matches!(child.kind(), "::" | "."))
        else {
            continue;
        };
        if operator.kind() != "::" {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, operator.byte_range())
                .corrected_by(Edit {
                    start: operator.start_byte(),
                    end: operator.end_byte(),
                    replacement: ".".to_owned(),
                    safe: true,
                }),
        );
    }
}
