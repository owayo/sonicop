//! `Style/ClassCheck`: `is_a?` and `kind_of?` are the same method, so a project picks one.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "is_a?".to_owned());

    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let current = context.source.node_text(selector);
        if !matches!(current, "is_a?" | "kind_of?") || current == style {
            continue;
        }
        let prefer = if current == "is_a?" {
            "kind_of?"
        } else {
            "is_a?"
        };
        offenses.push(
            context
                .offense(
                    format!("Prefer `Object#{prefer}` over `Object#{current}`."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: prefer.to_owned(),
                    safe: true,
                }),
        );
    }
}
