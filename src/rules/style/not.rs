use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `!` instead of `not`.";

/// `OPPOSITE_METHODS`: the comparisons a negation can be folded into.
fn opposite(method: &str) -> Option<&'static str> {
    match method {
        "==" => Some("!="),
        "!=" => Some("=="),
        "<=" => Some(">"),
        ">" => Some("<="),
        "<" => Some(">="),
        ">=" => Some("<"),
        _ => None,
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("unary") {
        let Some(selector) = node.field("operator") else {
            continue;
        };
        // `prefix_not?`: written as the keyword rather than as `!`.
        if context.source.node_text(selector) != "not" {
            continue;
        }
        let Some(receiver) = node.field("operand") else {
            continue;
        };
        // `range_with_surrounding_space(node.loc.selector, side: :right)`.
        let removed = selector.start_byte()
            ..super::ranges::extended_right(context.source.text(), selector.end_byte(), true);
        let offense = context.offense(MSG, selector.byte_range());

        let edits = match comparison_selector(context, receiver) {
            Some((operator, replacement)) => vec![
                Edit {
                    start: removed.start,
                    end: removed.end,
                    replacement: String::new(),
                    safe: true,
                },
                Edit {
                    start: operator.start_byte(),
                    end: operator.end_byte(),
                    replacement: replacement.to_owned(),
                    safe: true,
                },
            ],
            None if requires_parentheses(context, receiver) => vec![
                Edit {
                    start: removed.start,
                    end: removed.end,
                    replacement: "!(".to_owned(),
                    safe: true,
                },
                Edit {
                    start: node.end_byte(),
                    end: node.end_byte(),
                    replacement: ")".to_owned(),
                    safe: true,
                },
            ],
            None => vec![Edit {
                start: removed.start,
                end: removed.end,
                replacement: "!".to_owned(),
                safe: true,
            }],
        };
        offenses.push(offense.corrected_by_all(edits));
    }
}

/// `opposite_method?`: the operand is a comparison, whose selector can carry the negation instead.
fn comparison_selector<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    let selector = match node.kind_str() {
        "binary" => node.field("operator")?,
        "call" => node.field("method")?,
        _ => return None,
    };
    opposite(context.source.node_text(selector)).map(|replacement| (selector, replacement))
}

/// `requires_parens?`: `not a && b` and `not a + b` both bind the whole expression, which `!` does
/// not.
fn requires_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        // `operator_keyword?`: an `and` or `or` node.
        "boolean" => true,
        // `binary_operation?`: an operator method written infix.
        "binary" => node
            .field("operator")
            .is_some_and(|operator| {
                let text = context.source.node_text(operator);
                super::nodes::is_operator_method(text) || matches!(text, "and" | "or" | "&&" | "||")
            }),
        // `node.if_type? && node.ternary?`.
        "conditional" => true,
        _ => false,
    }
}
