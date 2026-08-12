use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::conditions::in_condition;

const MSG: &str = "Avoid the use of flip-flop operators.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // A range written where a condition is expected is an `iflipflop` or an `eflipflop` upstream
    // rather than a range, and the two are the only nodes this cop reports.
    for node in context.nodes_of("range") {
        if in_condition(node, context) {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}
