use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

const REPLACEMENT: &str = "add_dependency";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `RESTRICT_ON_SEND = %i[add_runtime_dependency]` plus `return if !node.receiver ||
        // node.arguments.empty?`. Nothing about the receiver is checked beyond its being there, so
        // `Foo.add_runtime_dependency 'x'` is reported as readily as a specification is.
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "add_runtime_dependency" {
            continue;
        }
        // Upstream defines only `on_send`, so a safe navigation call is a `csend` no handler sees.
        if node.field("receiver").is_none() || !is_plain_send(node, context) {
            continue;
        }
        if arguments(node).is_empty() {
            continue;
        }
        offenses.push(
            context
                .offense(
                    "Use `add_dependency` instead of `add_runtime_dependency`.",
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: REPLACEMENT.to_owned(),
                    safe: true,
                }),
        );
    }
}
