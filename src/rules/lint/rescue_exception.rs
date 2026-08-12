use super::rescue_clause::{body, const_name, end};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str =
    "Avoid rescuing the `Exception` class. Perhaps you meant to rescue `StandardError`?";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("rescue") {
        let Some(exceptions) = node.child_by_field_name("exceptions") else {
            continue;
        };
        let mut cursor = exceptions.walk();
        let targets_exception = exceptions
            .named_children(&mut cursor)
            .any(|exception| const_name(exception, context.source).as_deref() == Some("Exception"));
        if !targets_exception {
            continue;
        }
        let statements = body(node);
        offenses.push(context.offense(MSG, node.start_byte()..end(node, &statements)));
    }
}
