use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Place the first line of module body on its own line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("module") {
        super::trailing_body::check(context, offenses, node, MSG);
    }
}
