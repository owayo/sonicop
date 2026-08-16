use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

/// `"else".len()` and `"end".len()`, the two keywords whose spans are fixed.
const ELSE_LENGTH: usize = 4;
const END_LENGTH: usize = 3;

/// A negated condition with both branches written out, which reads better the other way round.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `@corrected_nodes`: only the outermost of a nest is rewritten, and the ones inside it are
    // reported without a correction.
    let mut corrected: HashSet<usize> = HashSet::new();
    for node in context.nodes_of_any(&["if", "unless", "conditional"]) {
        // `if_else?`: an `elsif` is a node of its own here, and an `else` that holds one is not a
        // plain else branch either.
        let Some(alternative) = node.field("alternative") else {
            continue;
        };
        if node.kind_str() != "conditional" && alternative.kind_str() != "else" {
            continue;
        }
        let Some(condition) = node.field("condition").map(unwrap_parentheses) else {
            continue;
        };
        let Some(inverted) = inverted_condition(condition, context) else {
            continue;
        };
        let ternary = node.kind_str() == "conditional";
        let type_name = if ternary { "ternary" } else { "if-else" };
        let offense = context.offense(
            format!("Invert the negated condition and swap the {type_name} branches."),
            node.byte_range(),
        );
        if ancestors(node).any(|ancestor| corrected.contains(&ancestor.id())) {
            offenses.push(offense);
            continue;
        }
        corrected.insert(node.id());
        let mut edits = vec![Edit {
            start: condition.start_byte(),
            end: condition.end_byte(),
            replacement: inverted,
            safe: true,
        }];
        edits.extend(swap_branches(node, alternative, ternary, context));
        offenses.push(offense.corrected_by_all(edits));
    }
}

/// `correct_negated_condition`: what the condition says once the negation is taken out.
fn inverted_condition(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "unary" => {
            if context.source.node_text(node.field("operator")?) != "!" {
                return None;
            }
            let operand = node.field("operand")?;
            // `double_negation?`: `!!x` is a way of casting to a boolean, not a negation to undo.
            if operand.kind_str() == "unary"
                && operand
                    .field("operator")
                    .is_some_and(|inner| context.source.node_text(inner) == "!")
            {
                return None;
            }
            Some(context.source.node_text(operand).to_owned())
        }
        // `NEGATED_EQUALITY_METHODS`.
        "binary" => {
            let inverted = match context.source.node_text(node.field("operator")?) {
                "!=" => "==",
                "!~" => "=~",
                _ => return None,
            };
            Some(format!(
                "{} {inverted} {}",
                context.source.node_text(node.field("left")?),
                context.source.node_text(node.field("right")?)
            ))
        }
        _ => None,
    }
}

/// `swap_branches`.
fn swap_branches(
    node: Node<'_>,
    alternative: Node<'_>,
    ternary: bool,
    context: &RuleContext<'_>,
) -> Vec<Edit> {
    let text = context.source.text();
    if ternary {
        let (Some(consequence), Some(other)) = (node.field("consequence"), Some(alternative))
        else {
            return Vec::new();
        };
        return vec![
            replace(
                consequence.byte_range(),
                text[other.byte_range()].to_owned(),
            ),
            replace(
                other.byte_range(),
                text[consequence.byte_range()].to_owned(),
            ),
        ];
    }
    // `node.if_branch.nil?`: with nothing between the condition and the `else`, the else keyword's
    // line goes away instead.
    if node
        .field("consequence")
        .is_none_or(|branch| super::nodes::children(branch).is_empty())
    {
        let keyword = alternative.start_byte()..alternative.start_byte() + ELSE_LENGTH;
        return vec![replace(support::whole_lines(keyword, context), String::new())];
    }
    let Some(condition) = node.field("condition") else {
        return Vec::new();
    };
    let if_range = condition.end_byte()..alternative.start_byte();
    let else_range = alternative.start_byte() + ELSE_LENGTH..node.end_byte() - END_LENGTH;
    if if_range.end < if_range.start || else_range.end < else_range.start {
        return Vec::new();
    }
    vec![
        replace(if_range.clone(), text[else_range.clone()].to_owned()),
        replace(else_range, text[if_range].to_owned()),
    ]
}

/// `unwrap_begin_nodes`.
fn unwrap_parentheses<'tree>(node: Node<'tree>) -> Node<'tree> {
    let mut current = node;
    while matches!(
        current.kind_str(),
        "parenthesized_statements" | "begin_block"
    ) {
        match super::nodes::children(current).as_slice() {
            [only] => current = *only,
            _ => break,
        }
    }
    current
}

fn replace(range: std::ops::Range<usize>, replacement: String) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement,
        safe: true,
    }
}

/// The ancestors of a node, innermost first.
fn ancestors<'tree>(node: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    std::iter::successors(node.parent(), |current| current.parent())
}
