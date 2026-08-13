//! `Layout/SpaceAfterMethodName`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not put a space between a method name and the opening parenthesis.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        // `args.parenthesized_call?`: only a parameter list written with parentheses.
        if parameters.child(0).is_none_or(|child| child.kind_str() != "(") {
            continue;
        }
        let start = parameters.start_byte();
        if start == 0 || text.as_bytes()[start - 1] != b' ' {
            continue;
        }
        offenses.push(context.offense(MSG, (start - 1)..start).corrected_by(Edit {
            start: start - 1,
            end: start,
            replacement: String::new(),
            safe: true,
        }));
    }
}
