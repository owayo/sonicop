use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["while", "until", "while_modifier", "until_modifier"]) {
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let Some(negated) = single_negative(context, condition) else {
            continue;
        };
        let Some(operand) = negated.child_by_field_name("operand") else {
            continue;
        };
        let Some(keyword) = super::conditional::token(node, &["while", "until"]) else {
            continue;
        };
        let current = context.source.node_text(keyword);
        let inverse = match current {
            "while" => "until",
            _ => "while",
        };
        let message = format!("Favor `{inverse}` over `{current}` for negative conditions.");
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by_all([
                    Edit {
                        start: keyword.start_byte(),
                        end: keyword.end_byte(),
                        replacement: inverse.to_owned(),
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

/// `(send !(send _ :!) :!)` after `condition.children.last while condition.begin_type?`, with the
/// empty parenthesized condition ruled out first.
pub(super) fn single_negative<'tree>(
    context: &RuleContext<'_>,
    mut condition: Node<'tree>,
) -> Option<Node<'tree>> {
    // `empty_condition?` is `(begin)`: `while ()` has nothing to negate.
    if condition.kind() == "parenthesized_statements"
        && super::nodes::children(condition).is_empty()
    {
        return None;
    }
    while condition.kind() == "parenthesized_statements" {
        condition = *super::nodes::children(condition).last()?;
    }
    if !is_negation(context, condition) {
        return None;
    }
    // `!(send _ :!)`: a doubled negation is not the shape this rewrites.
    let operand = condition.child_by_field_name("operand")?;
    (!is_negation(context, operand)).then_some(condition)
}

/// `(send _ :!)`, which is how the parser spells both `!x` and `not x`.
fn is_negation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "unary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}
