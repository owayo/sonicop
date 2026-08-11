use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // `on_def` leaves a method with no parameter list alone, and an empty one -- `def foo()`
        // -- counts as none.
        let Some(list) = node.child_by_field_name("parameters") else {
            continue;
        };
        if list.named_child_count() == 0 {
            continue;
        }
        super::uncommunicative::check(context, offenses, list, "method parameter");
    }
}
