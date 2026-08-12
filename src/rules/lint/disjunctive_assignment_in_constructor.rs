use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::named_children;

const MSG: &str = "Unnecessary disjunctive assignment. Use plain assignment.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("method") {
        let Some(name) = node.child_by_field_name("name") else {
            continue;
        };
        if context.source.node_text(name) != "initialize" {
            continue;
        }
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        // A body with a `rescue` or an `ensure` is one node of that type upstream rather than the
        // `begin` this walks, so the very first line already fails the `or_asgn` test.
        let lines = named_children(body);
        if lines
            .iter()
            .any(|line| matches!(line.kind(), "rescue" | "ensure" | "else"))
        {
            continue;
        }
        for line in lines {
            let Some(operator) = disjunctive_operator(line, context) else {
                break;
            };
            // Only an instance variable is certain to be unset in a constructor. Anything else
            // leaves the loop running: upstream breaks on the *type*, not on the check.
            if line
                .child_by_field_name("left")
                .is_none_or(|left| left.kind() != "instance_variable")
            {
                continue;
            }
            offenses.push(
                context
                    .offense(MSG, operator.byte_range())
                    .corrected_by(Edit {
                        start: operator.start_byte(),
                        end: operator.end_byte(),
                        replacement: "=".to_owned(),
                        safe: true,
                    }),
            );
        }
    }
}

fn disjunctive_operator<'tree>(
    line: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if line.kind() != "operator_assignment" {
        return None;
    }
    let operator = line.child_by_field_name("operator")?;
    (context.source.node_text(operator) == "||=").then_some(operator)
}
