use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::element_line_breaks::{ARRAYS, elements, line_breaks};

const MSG: &str = "Each item in a multi-line array must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    for node in context.nodes_of_any(ARRAYS) {
        offenses.extend(line_breaks(context, &elements(node, context), ignore_last, MSG));
    }
}
