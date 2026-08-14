//! `Layout/MultilineArrayLineBreaks`.

use super::element_line_breaks::{check_line_breaks, literal_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Each item in a multi-line array must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    for node in
        context.nodes_of_any(&["array", "string_array", "symbol_array", "right_assignment_list"])
    {
        check_line_breaks(context, MSG, &literal_elements(node), ignore_last, offenses);
    }
}
