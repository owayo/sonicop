use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::statements::statements;

const MSG: &str = "Avoid empty expressions.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_begin` reaches the `begin` node that `(...)` builds and the one an interpolation holds,
    // but never the `kwbegin` of `begin ... end`, which is a type of its own upstream.
    for node in context.nodes_of_any(&["parenthesized_statements", "interpolation"]) {
        if statements(node).is_empty() {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}
