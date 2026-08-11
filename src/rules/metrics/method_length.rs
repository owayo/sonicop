use super::support::{LengthTarget, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(10);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        report_length(context, offenses, node, max, "Method", LengthTarget::Method);
    }
}
