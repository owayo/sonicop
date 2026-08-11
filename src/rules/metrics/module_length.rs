use super::support::{HeredocEnds, LengthTarget, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(100);
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of("module") {
        report_length(
            context,
            offenses,
            node,
            max,
            "Module",
            LengthTarget::Classlike,
            &heredocs,
        );
    }
}
