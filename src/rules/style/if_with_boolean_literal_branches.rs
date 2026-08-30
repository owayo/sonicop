//! `Style/IfWithBooleanLiteralBranches`: a conditional whose branches are `true` and `false` is the
//! condition itself.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

/// `RuboCop::AST::Node::COMPARISON_OPERATORS`.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<", "<=>"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context
        .setting("AllowedMethods")
        .unwrap_or_else(|| vec!["infinite?".to_owned(), "nonzero?".to_owned()]);
    for node in context.nodes_of_any(&["if", "unless", "elsif", "conditional"]) {
        let (Some(condition), Some(consequence), Some(alternative)) = (
            node.field("condition"),
            branch(node.field("consequence")),
            branch(node.field("alternative")),
        ) else {
            continue;
        };
        // `<true false>`: the two branches are the two boolean literals, in either order.
        let (first, second) = (
            context.source.node_text(consequence),
            context.source.node_text(alternative),
        );
        if !matches!((first, second), ("true", "false") | ("false", "true")) {
            continue;
        }
        if !returns_boolean(condition, &allowed, context) {
            continue;
        }
        // `multiple_elsif?`: a chain of them is left to the pass that has already shortened it.
        if node.kind_str() == "elsif"
            && context
                .parent(node)
                .is_some_and(|parent| parent.kind_str() == "elsif")
        {
            continue;
        }
        let ternary = node.kind_str() == "conditional";
        let keyword = super::conditional::token(node, &["if", "unless", "elsif"]);
        // `offense_range_with_keyword`: a ternary has no keyword to point at, so the range runs
        // from the end of the condition instead.
        let range = if ternary {
            condition.end_byte()..node.end_byte()
        } else {
            match keyword {
                Some(keyword) => keyword.byte_range(),
                None => continue,
            }
        };
        let message = if node.kind_str() == "elsif" {
            "Use `else` instead of redundant `elsif` with boolean literal branches.".to_owned()
        } else {
            let named = if ternary {
                "ternary operator".to_owned()
            } else {
                format!("`{}`", context.source.node_text(keyword.unwrap()))
            };
            format!("Remove redundant {named} with boolean literal branches.")
        };
        // `opposite_condition?`: the branch reached when the condition holds says `false`.
        let opposite = match node.kind_str() {
            "unless" => first == "true",
            _ => first == "false",
        };
        let source = context.source.node_text(condition);
        let replacement = if opposite && requires_parentheses(condition, context) {
            format!("!({source})")
        } else if opposite {
            format!("!{source}")
        } else {
            source.to_owned()
        };
        // `insert_before(node, ...)` hangs the insertion off the whole `elsif`, not off the
        // keyword the offense was reported on, so the anchor has to say so.
        let mut offense = context.offense(message, range);
        let edits = if node.kind_str() == "elsif" {
            offense = offense.corrections_anchored_at(node.byte_range());
            vec![
                Edit {
                    start: node.start_byte(),
                    end: node.start_byte(),
                    replacement: "else\n".to_owned(),
                    safe: true,
                },
                Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!(
                        "{}{replacement}",
                        " ".repeat(consequence.start_position().column)
                    ),
                    safe: true,
                },
            ]
        } else {
            vec![Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }]
        };
        offenses.push(offense.corrected_by_all(edits));
    }
}

/// `return_boolean_value?`: whether the condition already answers `true` or `false`.
fn returns_boolean(node: Node<'_>, allowed: &[String], context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        // `(begin ...)`: a condition written in parentheses.
        "parenthesized_statements" => match super::nodes::children_in(node, context).as_slice() {
            [first, ..] => returns_boolean(*first, allowed, context),
            [] => false,
        },
        "binary" => {
            let operator = node
                .field("operator")
                .map(|operator| context.source.node_text(operator));
            let (Some(left), Some(right)) = (node.field("left"), node.field("right")) else {
                return false;
            };
            match operator {
                // `or`: both sides have to answer with a boolean.
                Some("||" | "or") => {
                    returns_boolean(left, allowed, context)
                        && returns_boolean(right, allowed, context)
                }
                // `and`: only the right-hand side is the value.
                Some("&&" | "and") => returns_boolean(right, allowed, context),
                _ => assume_boolean(node, allowed, context),
            }
        }
        _ => assume_boolean(node, allowed, context),
    }
}

/// `assume_boolean_value?`: a comparison, a predicate, or a doubled `!`.
fn assume_boolean(node: Node<'_>, allowed: &[String], context: &RuleContext<'_>) -> bool {
    // `double_negative?` is `(send (send _ :!) :!)`, and each `!` may be written either way.
    // Checking this before the `call` arm matters: `x.!.!` is a `call` whose method is `!`, which
    // the arm below would otherwise read as a predicate-looking method name.
    if let Some(found) = send_node::bang(node, context) {
        return send_node::bang(found.operand, context).is_some();
    }
    match node.kind_str() {
        "binary" => {
            let operator = context.source.node_text(match node.field("operator") {
                Some(operator) => operator,
                None => return false,
            });
            !allowed.iter().any(|entry| entry == operator)
                && COMPARISON_OPERATORS.contains(&operator)
        }
        "call" => {
            let name = context.source.node_text(match node.field("method") {
                Some(name) => name,
                None => return false,
            });
            if allowed.iter().any(|entry| entry == name) {
                return false;
            }
            COMPARISON_OPERATORS.contains(&name) || name.ends_with('?')
        }
        _ => false,
    }
}

/// `require_parentheses?`: `and`/`or` and a comparison both bind looser than the `!` in front.
fn requires_parentheses(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "binary" {
        return false;
    }
    let Some(operator) = node.field("operator") else {
        return false;
    };
    let operator = context.source.node_text(operator);
    matches!(operator, "&&" | "||" | "and" | "or") || COMPARISON_OPERATORS.contains(&operator)
}

/// The one statement a branch holds.
fn branch<'tree>(clause: Option<Node<'tree>>) -> Option<Node<'tree>> {
    let clause = clause?;
    match clause.kind_str() {
        "then" | "else" => match super::nodes::children(clause).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        "elsif" => None,
        _ => Some(clause),
    }
}
