use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, top_level_constant};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        let name = context.source.node_text(method);
        if !matches!(name, "load" | "restore") || !is_plain_send(node, context) {
            continue;
        }
        if !node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "Marshal", context))
        {
            continue;
        }
        // `(... $_ !(send (const {nil? cbase} :Marshal) :dump ...) _?)`: one argument that is not a
        // `Marshal.dump`, and at most one more. `Marshal.load` on its own carries no payload, and
        // three arguments are no call to `Marshal.load` at all.
        let arguments = arguments(node);
        if !(1..=2).contains(&arguments.len()) || marshal_dump(arguments[0].first(), context) {
            continue;
        }
        offenses.push(context.offense(
            format!("Avoid using `Marshal.{name}`."),
            method.byte_range(),
        ));
    }
}

/// The deep-copy hack `Marshal.load(Marshal.dump(x))`, which upstream exempts because the payload
/// it loads is one it just wrote itself.
fn marshal_dump(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind() == "call"
        && is_plain_send(node, context)
        && node
            .child_by_field_name("method")
            .is_some_and(|method| context.source.node_text(method) == "dump")
        && node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "Marshal", context))
}
