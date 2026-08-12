use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "both".to_owned());

    for node in context.nodes_of_any(&["unless", "unless_modifier"]) {
        let modifier = node.kind() == "unless_modifier";
        // `correct_style?`: each style leaves one of the two forms alone.
        if (style == "prefix" && modifier) || (style == "postfix" && !modifier) {
            continue;
        }
        // `return if node.if_type? && node.else?`: an `unless` with an `else` is left as it is.
        if node.child_by_field_name("alternative").is_some() {
            continue;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let Some(negated) = super::negated_while::single_negative(context, condition) else {
            continue;
        };
        let Some(operand) = negated.child_by_field_name("operand") else {
            continue;
        };
        let Some(keyword) = super::conditional::token(node, &["unless"]) else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    "Favor `if` over `unless` for negative conditions.",
                    node.byte_range(),
                )
                .corrected_by_all([
                    Edit {
                        start: keyword.start_byte(),
                        end: keyword.end_byte(),
                        replacement: "if".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: negated.start_byte(),
                        end: negated.end_byte(),
                        replacement: context.source.node_text(operand).to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}
