use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let verbose = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "verbose");

    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        // `node.arguments.one?`: the predicate takes exactly the key it asks about.
        let one_argument = node
            .field("arguments")
            .is_some_and(|arguments| super::nodes::children_in(arguments, context).len() == 1);
        if !one_argument {
            continue;
        }
        let current = context.source.node_text(selector);
        let preferred = match (verbose, current) {
            (true, "key?") => "has_key?",
            (true, "value?") => "has_value?",
            (false, "has_key?") => "key?",
            (false, "has_value?") => "value?",
            _ => continue,
        };
        offenses.push(
            context
                .offense(
                    format!("Use `Hash#{preferred}` instead of `Hash#{current}`."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: preferred.to_owned(),
                    safe: true,
                }),
        );
    }
}
