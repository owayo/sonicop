use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::node_equality::identical;

const MSG: &str = "Duplicate `rescue` exception detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // Upstream's `rescue` node holds every clause of one body, where the grammar gives each clause
    // a node of its own; the clauses that share a parent are the ones that share a `rescue`.
    let mut clauses: HashMap<usize, Vec<Node<'_>>> = HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    for node in context.nodes_of("rescue") {
        let Some(parent) = node.parent() else {
            continue;
        };
        let branches = clauses.entry(parent.id()).or_insert_with(|| {
            order.push(parent.id());
            Vec::new()
        });
        branches.push(node);
    }
    for parent in order {
        let mut seen: Vec<Node<'_>> = Vec::new();
        for clause in &clauses[&parent] {
            for exception in exceptions(*clause) {
                if seen
                    .iter()
                    .any(|earlier| identical(*earlier, exception, context))
                {
                    offenses.push(context.offense(MSG, exception.byte_range()));
                } else {
                    seen.push(exception);
                }
            }
        }
    }
}

/// `ResbodyNode#exceptions`. Upstream wraps them in an `array` however many were written, so the
/// list is simply what the clause names.
fn exceptions<'tree>(clause: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(list) = clause.child_by_field_name("exceptions") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor).collect()
}
