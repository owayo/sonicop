//! `Layout/MultilineHashKeyLineBreaks`.

use super::element_line_breaks::{check_line_breaks, literal_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Each key in a multi-line hash must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    // `starts_with_curly_brace?` and `node.loc.begin` both ask for the braces the grammar only
    // builds a `hash` node for.
    for node in context.nodes_of("hash") {
        check_line_breaks(context, MSG, &literal_elements(node), ignore_last, offenses);
    }
}
