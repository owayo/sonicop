use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::element_line_breaks::{children_line_break, elements};

const MSG: &str = "Add a line break before the first element of a multi-line hash.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    // `return unless node.loc.begin`: only a hash written with braces, which is the only one the
    // grammar gives a node of its own.
    for node in context.nodes_of("hash") {
        let children = elements(node, context);
        if children.is_empty() {
            continue;
        }
        offenses.extend(children_line_break(
            context, node, &children, ignore_last, MSG,
        ));
    }
}
