//! `Layout/FirstHashElementLineBreak`.

use super::element_line_breaks::{check_children_line_break, literal_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Add a line break before the first element of a multi-line hash.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    // `node.loc.begin`: only a hash written with braces, which is the only kind the grammar has a
    // `hash` node for.
    for node in context.nodes_of("hash") {
        let children = literal_elements(node);
        check_children_line_break(
            context,
            MSG,
            node.start_byte(),
            &children,
            ignore_last,
            offenses,
        );
    }
}
