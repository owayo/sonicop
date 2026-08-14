//! `Layout/FirstMethodParameterLineBreak`.

use super::element_line_breaks::{check_children_line_break, method_uses_parens};
use super::support::definition_parameters;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str =
    "Add a line break before the first parameter of a multi-line method parameter list.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let children = definition_parameters(node);
        let Some(first) = children.first() else {
            continue;
        };
        if !method_uses_parens(context, node.start_byte(), first.start) {
            continue;
        }
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
