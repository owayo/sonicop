//! `Style/NegatedIf`: `if !x` says with two tokens what `unless x` says with one.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "both".to_owned());

    for node in context.nodes_of_any(&["if", "if_modifier"]) {
        let modifier = node.kind() == "if_modifier";
        // `correct_style?`: each style leaves one of the two forms alone.
        if (style == "prefix" && modifier) || (style == "postfix" && !modifier) {
            continue;
        }
        // `return if node.if_type? && node.else?`.
        if !modifier && has_else(node) {
            continue;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let Some(negation) = single_negative(context, condition) else {
            continue;
        };
        let Some(operand) = negation.child_by_field_name("operand") else {
            continue;
        };
        let Some(keyword) = super::conditional::token(node, &["if"]) else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    "Favor `unless` over `if` for negative conditions.",
                    node.byte_range(),
                )
                .corrected_by_all([
                    Edit {
                        start: keyword.start_byte(),
                        end: keyword.end_byte(),
                        replacement: "unless".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: negation.start_byte(),
                        end: negation.end_byte(),
                        replacement: context.source.node_text(operand).to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `single_negative?`: the condition, once its parentheses are peeled off, is one `!` applied to
/// something that is not itself a negation. `not x` is the same `send` upstream, so it counts too.
fn single_negative<'tree>(
    context: &RuleContext<'_>,
    condition: Node<'tree>,
) -> Option<Node<'tree>> {
    let mut current = condition;
    // `condition = condition.children.last while condition.begin_type?`, with `(begin)` -- an
    // empty condition -- excluded before the loop even starts.
    while current.kind() == "parenthesized_statements" {
        current = *super::nodes::children(current).last()?;
    }
    if current.kind() != "unary" {
        return None;
    }
    let operator = current.child_by_field_name("operator")?;
    if !matches!(context.source.node_text(operator), "!" | "not") {
        return None;
    }
    let operand = current.child_by_field_name("operand")?;
    // `!(send _ :!)`: `!!x` negates a negation and is left alone.
    if operand.kind() == "unary"
        && operand
            .child_by_field_name("operator")
            .is_some_and(|inner| matches!(context.source.node_text(inner), "!" | "not"))
    {
        return None;
    }
    Some(current)
}

fn has_else(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "else" | "elsif"))
}
