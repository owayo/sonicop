//! `Layout/MultilineMethodArgumentLineBreaks`.

use super::element_line_breaks::{call_arguments, check_line_breaks};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Each argument in a multi-line method call must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    for node in context.nodes_of("call") {
        // `node.method?(:[]=)`: an index assignment is a different shape here anyway.
        if node
            .field("method")
            .is_some_and(|name| context.source.node_text(name) == "[]=")
        {
            continue;
        }
        check_line_breaks(context, MSG, &call_arguments(node), ignore_last, offenses);
    }
}
