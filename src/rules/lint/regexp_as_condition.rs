use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str =
    "Do not use regexp literal as a condition. The regexp literal matches `$_` implicitly.";

/// The node kinds `Node#conditional?` names, which is what has to stand somewhere above the match.
const CONDITIONALS: &[&str] = &[
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

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for regexp in context.nodes_of("regex") {
        if !is_current_line_match(regexp, context) || !has_conditional_ancestor(regexp) {
            continue;
        }
        let source = context.source.node_text(regexp);
        // `!` binds tighter than `=~`, so the rewritten match has to be parenthesised when the
        // literal was the operand of one.
        let negation = regexp
            .parent()
            .filter(|parent| is_negation(*parent, context));
        let edit = match negation {
            Some(parent) => Edit {
                start: parent.start_byte(),
                end: parent.end_byte(),
                replacement: format!("!({source} =~ $_)"),
                safe: true,
            },
            None => Edit {
                start: regexp.start_byte(),
                end: regexp.end_byte(),
                replacement: format!("{source} =~ $_"),
                safe: true,
            },
        };
        offenses.push(context.offense(MSG, regexp.byte_range()).corrected_by(edit));
    }
}

/// Whether the parser would have wrapped the literal in a `match_current_line`, which it does for
/// the condition of an `if`, a `while` or an `until` and for the operand of a `!` -- reaching
/// through parentheses, `and`/`or` and a flip-flop range on the way.
fn is_current_line_match(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    loop {
        let Some(parent) = current.parent() else {
            return false;
        };
        if is_negation(parent, context) {
            return true;
        }
        if is_condition_of(parent, current) {
            return true;
        }
        let carries = match parent.kind() {
            // `(cond)` is a `begin` upstream, and only one holding a single statement recurses.
            "parenthesized_statements" => super::statements::statements(parent).len() == 1,
            "binary" => parent
                .child_by_field_name("operator")
                .is_some_and(|operator| {
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
    CONDITIONALS.contains(&parent.kind())
        && parent
            .child_by_field_name("condition")
            .is_some_and(|condition| condition.id() == child.id())
}

/// `!x` and `not x`, the two forms `check_condition` is called for outside a conditional.
fn is_negation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind() == "unary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}

fn has_conditional_ancestor(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if CONDITIONALS.contains(&ancestor.kind()) {
            return true;
        }
        current = ancestor.parent();
    }
    false
}
