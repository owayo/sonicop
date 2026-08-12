use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::node_equality::identical;

const MSG: &str = "Duplicate `when` condition detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for case in context.nodes_of("case") {
        let mut seen: Vec<Node<'_>> = Vec::new();
        let mut cursor = case.walk();
        let branches: Vec<Node<'_>> = case
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "when")
            .collect();
        for branch in branches {
            for condition in conditions(branch) {
                if seen
                    .iter()
                    .any(|earlier| identical(*earlier, condition, context))
                {
                    offenses.push(context.offense(MSG, condition.byte_range()));
                } else {
                    seen.push(condition);
                }
            }
        }
    }
}

/// `WhenNode#conditions`. The grammar wraps each one in a `pattern` node that spans exactly the
/// condition, so what upstream reports is the node inside it.
fn conditions<'tree>(branch: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = branch.walk();
    branch
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "pattern")
        .filter_map(|pattern| pattern.named_child(0))
        .collect()
}
