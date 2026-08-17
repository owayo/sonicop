use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["while", "until", "while_modifier", "until_modifier"]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let Some(negated) = single_negative(context, condition) else {
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
                        start: negated.node.start_byte(),
                        end: negated.node.end_byte(),
                        replacement: context.source.node_text(negated.operand).to_owned(),
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
) -> Option<send_node::Negation<'tree>> {
    // `empty_condition?` is `(begin)`: `while ()` has nothing to negate.
    if condition.kind_str() == "parenthesized_statements"
        && super::nodes::children(condition).is_empty()
    {
        return None;
    }
    while condition.kind_str() == "parenthesized_statements" {
        condition = *super::nodes::children(condition).last()?;
    }
    let found = send_node::negation(condition, context)?;
    // `!(send _ :!)`: a doubled negation is not the shape this rewrites. **The inner one may be
    // written `x.!`**, which is the same `(send _ :!)` upstream -- missing it turns `!x.!` into a
    // single negation and the cop reports what upstream leaves alone.
    send_node::negation(found.operand, context)
        .is_none()
        .then_some(found)
}
