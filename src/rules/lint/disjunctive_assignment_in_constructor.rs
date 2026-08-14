use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::named_children;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Unnecessary disjunctive assignment. Use plain assignment.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("method") {
        let Some(name) = node.field("name") else {
            continue;
        };
        if context.source.node_text(name) != "initialize" {
            continue;
        }
        let Some(body) = node.field("body") else {
            continue;
        };
        // A body with a `rescue` or an `ensure` is one node of that type upstream rather than the
        // `begin` this walks, so the very first line already fails the `or_asgn` test.
        let lines = named_children(body);
        if lines
            .iter()
            .any(|line| matches!(line.kind_str(), "rescue" | "ensure" | "else"))
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
                .field("left")
                .is_none_or(|left| left.kind_str() != "instance_variable")
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
    if line.kind_str() != "operator_assignment" {
        return None;
    }
    let operator = line.field("operator")?;
    (context.source.node_text(operator) == "||=").then_some(operator)
}
