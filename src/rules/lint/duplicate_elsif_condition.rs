use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::node_equality::identical;

const MSG: &str = "Duplicate `elsif` condition detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("if") {
        let mut seen: Vec<Node<'_>> = Vec::new();
        let mut current = Some(node);
        // `while node.if? || node.elsif?`: the chain an `if` opens, which upstream walks as nested
        // `if` nodes. An `unless` has neither keyword and never enters the loop.
        while let Some(link) = current {
            let Some(condition) = link.child_by_field_name("condition") else {
                break;
            };
            if seen
                .iter()
                .any(|earlier| identical(*earlier, condition, context))
            {
                offenses.push(context.offense(MSG, condition.byte_range()));
            }
            seen.push(condition);
            current = link
                .child_by_field_name("alternative")
                .filter(|alternative| alternative.kind() == "elsif");
        }
    }
}
