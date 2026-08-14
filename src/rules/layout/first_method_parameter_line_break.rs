use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::element_line_breaks::{elements, method_line_break};

const MSG: &str =
    "Add a line break before the first parameter of a multi-line method parameter list.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        let children = elements(parameters, context);
        if children.is_empty() {
            continue;
        }
        offenses.extend(method_line_break(
            context, node, &children, ignore_last, MSG,
        ));
    }
}
