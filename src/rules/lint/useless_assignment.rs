use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::{RuleContext, walk_named};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let root = context.root_node();
    for assignment in context.nodes_of("assignment") {
        let Some(left) = assignment.child_by_field_name("left") else {
            continue;
        };
        if left.kind() != "identifier" {
            continue;
        }
        let name = context.source.node_text(left);
        if name.starts_with('_') {
            continue;
        }
        if assignment.parent().is_some_and(|parent| {
            parent.kind() == "assignment"
                && parent
                    .child_by_field_name("right")
                    .is_some_and(|right| right.byte_range() == assignment.byte_range())
        }) {
            continue;
        }
        let scope = enclosing_scope(assignment).unwrap_or(root);
        if has_other_read(scope, assignment, name, context) {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Useless assignment to variable - `{name}`."),
                    left.byte_range(),
                )
                .corrected_by(Edit {
                    start: assignment.start_byte(),
                    end: assignment
                        .child_by_field_name("right")
                        .map_or(left.end_byte(), |right| right.start_byte()),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

fn enclosing_scope(mut node: Node<'_>) -> Option<Node<'_>> {
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "method" | "singleton_method") {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn has_other_read(
    scope: Node<'_>,
    assignment: Node<'_>,
    name: &str,
    context: &RuleContext<'_>,
) -> bool {
    let mut found = false;
    walk_named(scope, &mut |candidate| {
        if found {
            return;
        }
        if candidate.kind() == "pair"
            && context.source.node_text(candidate).trim() == format!("{name}:")
        {
            found = true;
            return;
        }
        if candidate.kind() == "call"
            && candidate
                .child_by_field_name("method")
                .is_some_and(|method| context.source.node_text(method) == "binding")
        {
            found = true;
            return;
        }
        if candidate.kind() == "identifier" && context.source.node_text(candidate) == "binding" {
            found = true;
            return;
        }
        if candidate.kind() != "identifier" || context.source.node_text(candidate) != name {
            return;
        }
        let is_this_assignment = candidate.byte_range()
            == assignment
                .child_by_field_name("left")
                .map_or(assignment.byte_range(), |left| left.byte_range());
        let is_write = candidate.parent().is_some_and(|parent| {
            parent.kind() == "assignment"
                && parent
                    .child_by_field_name("left")
                    .is_some_and(|left| left.byte_range() == candidate.byte_range())
        });
        found = !is_this_assignment && !is_write;
    });
    found
}
