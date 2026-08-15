use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::element_line_breaks::{elements, line_breaks};

const MSG: &str = "Each key in a multi-line hash must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    // `starts_with_curly_brace?` and `node.loc.begin` both ask for the braces the grammar only
    // writes a `hash` node for anyway.
    for node in context.nodes_of("hash") {
        offenses.extend(line_breaks(context, &elements(node, context), ignore_last, MSG));
    }
}
