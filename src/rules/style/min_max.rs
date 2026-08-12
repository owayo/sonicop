use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["array", "return", "right_assignment_list"]) {
        let elements = match node.kind() {
            // A `return` holds its values in an argument list the parser has no node for.
            "return" => match super::nodes::children(node).as_slice() {
                [list] if list.kind() == "argument_list" => super::nodes::children(*list),
                _ => continue,
            },
            _ => super::nodes::children(node),
        };
        let [low, high] = elements.as_slice() else {
            continue;
        };
        let (Some(receiver), Some(other)) = (
            call_receiver(context, *low, "min"),
            call_receiver(context, *high, "max"),
        ) else {
            continue;
        };
        // `[$_receiver !nil?]` used twice: the two calls have to be on the same thing.
        let receiver_source = context.source.node_text(receiver);
        if receiver_source != context.source.node_text(other) {
            continue;
        }
        // `offending_range`: the brackets belong to an array literal but a `return` has none.
        let offender = match node.kind() {
            "return" => low.start_byte()..high.end_byte(),
            _ => node.byte_range(),
        };
        let message = format!(
            "Use `{receiver_source}.minmax` instead of `{}`.",
            &context.source.text()[offender.clone()]
        );
        offenses.push(
            context
                .offense(message, offender.clone())
                .corrected_by(Edit {
                    start: offender.start,
                    end: offender.end,
                    replacement: format!("{receiver_source}.minmax"),
                    safe: true,
                }),
        );
    }
}

/// `(send [$_receiver !nil?] :name)`: the call has a receiver, that name and no arguments.
fn call_receiver<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    name: &str,
) -> Option<Node<'tree>> {
    if node.kind() != "call" || node.child_by_field_name("block").is_some() {
        return None;
    }
    if node.child_by_field_name("arguments").is_some() {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    (context.source.node_text(method) == name).then(|| node.child_by_field_name("receiver"))?
}
