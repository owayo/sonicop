use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Avoid the use of `BEGIN` blocks.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("begin_block") {
        let Some(keyword) = node.child(0) else {
            continue;
        };
        offenses.push(context.offense(MSG, keyword.byte_range()));
    }
}
