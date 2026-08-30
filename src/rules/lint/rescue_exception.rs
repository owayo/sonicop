use super::rescue_clause::{body, const_name, end};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

const MSG: &str =
    "Avoid rescuing the `Exception` class. Perhaps you meant to rescue `StandardError`?";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("rescue") {
        let Some(exceptions) = node.field("exceptions") else {
            continue;
        };
        let _cursor = exceptions.walk();
        let targets_exception = named_children_of(exceptions, context)
            .into_iter()
            .any(|exception| const_name(exception, context.source).as_deref() == Some("Exception"));
        if !targets_exception {
            continue;
        }
        let statements = body(node);
        offenses.push(context.offense(MSG, node.start_byte()..end(node, &statements)));
    }
}
