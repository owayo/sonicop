use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::element_line_breaks::{elements, line_breaks};

const MSG: &str = "Each parameter in a multi-line method definition must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        offenses.extend(line_breaks(context, &elements(parameters, context), ignore_last, MSG));
    }
}
