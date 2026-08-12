use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::is_plain_send;

const MSG: &str = "`BigDecimal.new()` is deprecated. Use `BigDecimal()` instead.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(method) != "new" || !is_plain_send(node, context) {
            continue;
        }
        let Some(receiver) = node.child_by_field_name("receiver") else {
            continue;
        };
        // `(const ${nil? cbase} :BigDecimal)`: the capture is the `::` that `::BigDecimal` is
        // written with, which the correction removes along with the method call.
        let cbase = match receiver.kind() {
            "constant" if context.source.node_text(receiver) == "BigDecimal" => None,
            "scope_resolution"
                if receiver.child_by_field_name("scope").is_none()
                    && receiver
                        .child_by_field_name("name")
                        .is_some_and(|name| context.source.node_text(name) == "BigDecimal") =>
            {
                receiver.child(0)
            }
            _ => continue,
        };
        let Some(dot) = node.child_by_field_name("operator") else {
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
