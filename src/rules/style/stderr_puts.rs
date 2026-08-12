//! `Style/StderrPuts`: `warn` writes the same place and can be silenced.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, top_level_constant};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(selector) != "puts" || !is_plain_send(node, context) {
            continue;
        }
        let Some(receiver) = node.child_by_field_name("receiver") else {
            continue;
        };
        let stream = context.source.node_text(receiver);
        let is_stderr = (receiver.kind() == "global_variable" && stream == "$stderr")
            || top_level_constant(receiver, "STDERR", context);
        if !is_stderr {
            continue;
        }
        // `:puts $_ ...`: the pattern needs at least one argument.
        if arguments(node).is_empty() {
            continue;
        }
        let range = node.start_byte()..selector.end_byte();
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `warn` instead of `{stream}.puts` to allow such output to be disabled."
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: "warn".to_owned(),
                    safe: true,
                }),
        );
    }
}
