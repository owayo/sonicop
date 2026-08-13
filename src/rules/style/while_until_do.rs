use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["while", "until"]) {
        if node.start_position().row == node.end_position().row {
            continue;
        }
        let Some(body) = node.field("body") else {
            continue;
        };
        // `node.do?`: the `do` was written. The grammar spells the body as a `do` node either way.
        let Some(keyword) = body.child(0).filter(|child| child.kind_str() == "do") else {
            continue;
        };
        // `same_line_body?`: `while x do y` keeps its `do`, whatever follows on the next lines.
        if super::nodes::children(body)
            .first()
            .is_some_and(|first| first.start_position().row == keyword.start_position().row)
        {
            continue;
        }
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let message = format!("Do not use `do` with multi-line `{}`.", node.kind_str());
        offenses.push(
            context
                .offense(message, keyword.byte_range())
                .corrected_by(Edit {
                    start: condition.end_byte(),
                    end: keyword.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
