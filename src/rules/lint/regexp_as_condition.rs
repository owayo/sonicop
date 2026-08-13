use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::conditions::{has_conditional_ancestor, in_condition};
use crate::rules::node_ext::NodeExt;

const MSG: &str =
    "Do not use regexp literal as a condition. The regexp literal matches `$_` implicitly.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for regexp in context.nodes_of("regex") {
        if !in_condition(regexp, context) || !has_conditional_ancestor(regexp) {
            continue;
        }
        let source = context.source.node_text(regexp);
        // `!` binds tighter than `=~`, so the rewritten match has to be parenthesised when the
        // literal was the operand of one.
        let negation = regexp.parent().filter(|parent| is_negation(*parent, context));
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

/// `!x` and `not x`, the two forms the parser calls `check_condition` for outside a conditional.
fn is_negation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "unary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}
