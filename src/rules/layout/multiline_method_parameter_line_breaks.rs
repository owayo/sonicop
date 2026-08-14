//! `Layout/MultilineMethodParameterLineBreaks`.

use super::element_line_breaks::check_line_breaks;
use super::support::definition_parameters;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Each parameter in a multi-line method definition must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        check_line_breaks(
            context,
            MSG,
            &definition_parameters(node),
            ignore_last,
            offenses,
        );
    }
}
