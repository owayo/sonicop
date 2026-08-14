//! `Layout/FirstMethodArgumentLineBreak`.

use super::element_line_breaks::{call_arguments, check_children_line_break, method_uses_parens};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str =
    "Add a line break before the first argument of a multi-line method argument list.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    for node in context.nodes_of_any(&["call", "super"]) {
        if node
            .field("method")
            .is_some_and(|name| allowed.iter().any(|entry| entry == context.source.node_text(name)))
        {
            continue;
        }
        let children = call_arguments(node);
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
