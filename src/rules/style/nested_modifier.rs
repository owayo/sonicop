use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Avoid using nested modifiers.";

/// The node kinds a conditional written in modifier form comes out as.
const MODIFIERS: &[&str] = &[
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

/// `COMPARISON_OPERATORS`: what forces the inner condition to keep its own parentheses.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut ignored: Vec<std::ops::Range<usize>> = Vec::new();
    for node in context.nodes_of_any(MODIFIERS) {
        if ignored
            .iter()
            .any(|range| range.start <= node.start_byte() && node.end_byte() <= range.end)
        {
            continue;
        }
        let Some(outer) = node
            .parent_of(context)
            .filter(|parent| MODIFIERS.contains(&parent.kind_str()))
        else {
            continue;
        };
        let Some(keyword) = keyword(node) else {
            continue;
        };
        let mut offense = context.offense(MSG, keyword.byte_range());
        // Only a pair of `if`/`unless` is rewritten; nothing joins two loops into one condition.
        if is_conditional(node) && is_conditional(outer) {
            offense = offense.corrected_by(rewrite(context, node, outer, keyword));
        }
        offenses.push(offense);
        ignored.push(node.byte_range());
    }
}

fn is_conditional(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "if_modifier" | "unless_modifier")
}

/// The keyword the modifier is written with, which is the token before its condition.
fn keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("condition")?.prev_sibling()
}

/// `new_expression`: the two conditions joined into one, written over the inner keyword and the
/// outer condition.
fn rewrite(context: &RuleContext<'_>, inner: Node<'_>, outer: Node<'_>, keyword: Node<'_>) -> Edit {
    let outer_keyword = match outer.kind_str() {
        "unless_modifier" => "unless",
        _ => "if",
    };
    let operator = match outer_keyword {
        "if" => "&&",
        _ => "||",
    };
    let outer_condition = outer
        .field("condition")
        .expect("a modifier always has a condition");
    let left = {
        let source = context.source.node_text(outer_condition);
        match is_or(context, outer_condition) && operator == "&&" {
            true => format!("({source})"),
            false => source.to_owned(),
        }
    };
    Edit {
        start: keyword.start_byte(),
        end: outer_condition.end_byte(),
        replacement: format!(
            "{outer_keyword} {left} {operator} {}",
            right_hand_operand(context, inner, outer_keyword)
        ),
        safe: true,
    }
}

fn right_hand_operand(context: &RuleContext<'_>, inner: Node<'_>, outer_keyword: &str) -> String {
    let condition = inner
        .field("condition")
        .expect("a modifier always has a condition");
    let mut expression = match parenthesize_arguments(context, condition) {
        Some(rewritten) => rewritten,
        None => context.source.node_text(condition).to_owned(),
    };
    if is_or(context, condition) || is_comparison(context, condition) {
        expression = format!("({expression})");
    }
    let inner_keyword = match inner.kind_str() {
        "unless_modifier" => "unless",
        _ => "if",
    };
    match outer_keyword == inner_keyword {
        true => expression,
        false => format!("!{expression}"),
    }
}

/// `add_parentheses_to_method_arguments`: a call written without parentheses gets them, so the
/// joined condition still reads as one operand.
fn parenthesize_arguments(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    if node.kind_str() != "call" || node.field("block").is_some() {
        return None;
    }
    let selector = node.field("method")?;
    if selector.kind_str() == "operator" {
        return None;
    }
    let arguments = super::nodes::children(node.field("arguments")?);
    if arguments.is_empty() {
        return None;
    }
    let receiver = node
        .field("receiver")
        .map(|receiver| format!("{}.", context.source.node_text(receiver)))
        .unwrap_or_default();
    Some(format!(
        "{receiver}{}({})",
        context.source.node_text(selector),
        arguments
            .iter()
            .map(|argument| context.source.node_text(*argument))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn is_or(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "||" | "or"))
}

/// Whether upstream would find a comparison operator among the node's children, which is what a
/// `send` spelling one has.
fn is_comparison(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "binary" => node
            .field("operator")
            .is_some_and(|operator| {
                COMPARISON_OPERATORS.contains(&context.source.node_text(operator))
            }),
        "call" => node.field("method").is_some_and(|selector| {
            selector.kind_str() == "operator"
                && COMPARISON_OPERATORS.contains(&context.source.node_text(selector))
        }),
        _ => false,
    }
}
