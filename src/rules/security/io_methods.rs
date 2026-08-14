use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, is_string, send_range, string_text};

/// `RESTRICT_ON_SEND`: the `IO` methods that read a first argument as a command line when it opens
/// with a pipe.
const METHODS: &[&str] = &[
    "read",
    "binread",
    "write",
    "binwrite",
    "foreach",
    "readlines",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(method);
        if !METHODS.contains(&name) || !is_plain_send(node, context) {
            continue;
        }
        // `receiver.source == 'IO'`, compared as text: `::IO` reads as `::IO` and is left alone.
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        if context.source.node_text(receiver) != "IO" {
            continue;
        }
        // A first argument that opens with `|` names a command to run rather than a file to read,
        // and `File` has no such reading -- so the two are not interchangeable there.
        if let Some(argument) = arguments(node).first().map(|argument| argument.first())
            && is_string(argument, context)
            && string_text(argument, context).trim().starts_with('|')
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("`File.{name}` is safer than `IO.{name}`."),
                    send_range(node, context),
                )
                .corrected_by(Edit {
                    start: receiver.start_byte(),
                    end: receiver.end_byte(),
                    replacement: "File".to_owned(),
                    safe: true,
                }),
        );
    }
}
