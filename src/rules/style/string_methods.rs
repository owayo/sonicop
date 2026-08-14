use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The values of `PreferredMethods` in the bundled default configuration.
const DEFAULT_PREFERENCES: [&str; 1] = ["to_sym"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(preferences) =
        super::method_preference::preferred_methods(context, &DEFAULT_PREFERENCES)
    else {
        return;
    };
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let current = context.source.node_text(selector);
        let Some(prefer) = preferences.get(current) else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    format!("Prefer `{prefer}` over `{current}`."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: prefer.clone(),
                    safe: true,
                }),
        );
    }
}
