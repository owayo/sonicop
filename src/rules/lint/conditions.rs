//! Where `Builders::Default#check_condition` reaches, which is what turns two literals into nodes
//! of their own: a regexp becomes a `match_current_line` and a range becomes a flip-flop.
//!
//! The parser applies it to the condition of an `if`, a `while` and an `until`, and to the operand
//! of a `!` -- then recurses through a parenthesised expression holding a single statement, through
//! `and`/`or`, and through both ends of a flip-flop.

use tree_sitter::Node;

use crate::rules::RuleContext;

use super::statements::statements;
use crate::rules::node_ext::NodeExt;

/// The node kinds `Node#conditional?` names.
pub(super) const CONDITIONALS: &[&str] = &[
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
    "conditional",
    "case",
    "case_match",
];

/// Whether the parser would have handed this node to `check_condition`.
pub(super) fn in_condition(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    loop {
        let Some(parent) = current.parent_of(context) else {
            return false;
        };
        if is_negation(parent, context) || is_condition_of(parent, current) {
            return true;
        }
        let carries = match parent.kind_str() {
            // `(cond)` is a `begin` upstream, and only one holding a single statement recurses.
            "parenthesized_statements" => statements(parent).len() == 1,
            "binary" => parent.field("operator").is_some_and(|operator| {
                matches!(
                    context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                )
            }),
            // A range in a condition is a flip-flop, whose two ends are conditions of their own.
            "range" => true,
            _ => false,
        };
        if !carries {
            return false;
        }
        current = parent;
    }
}

/// Whether `child` is what the parser handed to `check_condition` for `parent`.
fn is_condition_of(parent: Node<'_>, child: Node<'_>) -> bool {
    CONDITIONALS.contains(&parent.kind_str())
        && parent
            .field("condition")
            .is_some_and(|condition| condition.id() == child.id())
}

/// `!x` and `not x`, the two forms `check_condition` is called for outside a conditional.
fn is_negation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "unary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}

/// Whether any ancestor is one of the conditionals, which is `node.ancestors.any?(&:conditional?)`.
pub(super) fn has_conditional_ancestor(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if CONDITIONALS.contains(&ancestor.kind_str()) {
            return true;
        }
        current = ancestor.parent();
    }
    false
}
