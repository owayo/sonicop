use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::is_plain_send;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "`BigDecimal.new()` is deprecated. Use `BigDecimal()` instead.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "new" || !is_plain_send(node, context) {
            continue;
        }
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        // `(const ${nil? cbase} :BigDecimal)`: the capture is the `::` that `::BigDecimal` is
        // written with, which the correction removes along with the method call.
        let cbase = match receiver.kind_str() {
            "constant" if context.source.node_text(receiver) == "BigDecimal" => None,
            "scope_resolution"
                if receiver.field("scope").is_none()
                    && receiver
                        .field("name")
                        .is_some_and(|name| context.source.node_text(name) == "BigDecimal") =>
            {
                receiver.child(0)
            }
            _ => continue,
        };
        let Some(dot) = node.field("operator") else {
            continue;
        };
        // Three separate `remove` calls upstream, and three here: one replacement spanning them
        // would swallow whatever else wants to correct the receiver in between.
        let mut edits = vec![remove(method.byte_range()), remove(dot.byte_range())];
        if let Some(cbase) = cbase {
            edits.push(remove(cbase.byte_range()));
        }
        offenses.push(
            context
                .offense(MSG, method.byte_range())
                .corrected_by_all(edits),
        );
    }
}

fn remove(range: std::ops::Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}
