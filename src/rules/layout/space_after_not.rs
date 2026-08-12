//! `Layout/SpaceAfterNot`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Do not leave space between `!` and its argument.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for node in context.nodes_of("unary") {
        let Some(operator) = node.child(0) else {
            continue;
        };
        // `prefix_bang?` asks the selector to be `!` itself: `not x` is the same `send :!`
        // upstream but spells its selector `not`, and `x.!` has the operator behind its receiver.
        if &text[operator.byte_range()] != "!" || operator.start_byte() != node.start_byte() {
            continue;
        }
        let Some(operand) = node.child_by_field_name("operand") else {
            continue;
        };
        // `whitespace_after_operator?`: the receiver starts more than one character in.
        if operand.start_byte() <= node.start_byte() + 1 {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: operator.end_byte(),
            end: operand.start_byte(),
            replacement: String::new(),
            safe: true,
        }));
    }
}
