//! `Style/Proc`: `Proc.new { }` is `proc { }` spelled the long way.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range, top_level_constant};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `proc` instead of `Proc.new`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `(any_block $(send (const {nil? cbase} :Proc) :new) ...)`: without a block the call is
        // not a proc literal at all, and `csend` is a node type the pattern never matches.
        if node.field("block").is_none() || !is_plain_send(node, context) {
            continue;
        }
        let Some(method) = node.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "new" {
            continue;
        }
        // `:new` with nothing after it in the pattern means the call takes no arguments.
        if !arguments(node).is_empty() {
            continue;
        }
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        if !top_level_constant(receiver, "Proc", context) {
            continue;
        }
        let range = send_range(node, context);
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: "proc".to_owned(),
            safe: true,
        }));
    }
}
