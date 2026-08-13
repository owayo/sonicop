use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not use `then` for multiline `when` statement.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("when") {
        let Some(body) = node.field("body") else {
            continue;
        };
        let Some(then) = super::conditional::token(body, &["then"]) else {
            continue;
        };
        let conditions: Vec<_> = super::nodes::children(node)
            .into_iter()
            .filter(|child| child.kind_str() == "pattern")
            .collect();
        // `conditions.first.first_line == conditions.last.last_line`: a condition list spread over
        // lines needs the `then` to close it.
        let spread = match (conditions.first(), conditions.last()) {
            (Some(first), Some(last)) => first.start_position().row != last.end_position().row,
            _ => true,
        };
        if spread {
            continue;
        }
        // `same_line?(when_node, when_node.body)`: a body on the `when`'s own line needs it too.
        if super::nodes::children(body)
            .first()
            .is_some_and(|first| first.start_position().row == node.start_position().row)
        {
            continue;
        }
        offenses.push(context.offense(MSG, then.byte_range()).corrected_by(Edit {
            start: super::ranges::extended_left(context.source.text(), then.start_byte(), false),
            end: then.end_byte(),
            replacement: String::new(),
            safe: true,
        }));
    }
}
