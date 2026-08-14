use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Prefer `Object#__send__` or `Object#public_send` to `send`.";

/// `RESTRICT_ON_SEND = %i[send]` with `node.arguments?`: the receiver is never looked at, so a
/// receiverless `send(:foo)` counts too.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "send" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        if arguments.is_empty() {
            continue;
        }
        offenses.push(context.offense(MSG, selector.byte_range()));
    }
}
