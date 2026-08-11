use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::{nodes, trailing_comma};

const KIND: &str = "parameter of %<article>s method call";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((items, end)) = argument_span(node) else {
            continue;
        };
        let Some(last) = items.last() else {
            continue;
        };
        trailing_comma::check(
            context,
            &items,
            KIND,
            last.end_byte(),
            end,
            offenses,
        );
    }
}

/// The arguments upstream's `on_send` inspects, and the end of the range it searches for the comma.
///
/// Only a parenthesized call and an index read qualify: anything else has no bracket for the comma
/// to stand in front of.
fn argument_span<'tree>(node: Node<'tree>) -> Option<(Vec<Node<'tree>>, usize)> {
    match node.kind() {
        "element_reference" => {
            // `a[1] = 2` is a call to `[]=` upstream, whose name fails the `[]` test.
            if is_assignment_target(node) {
                return None;
            }
            let mut items = nodes::children(node);
            if node.child_by_field_name("object").is_some() && !items.is_empty() {
                items.remove(0);
            }
            Some((items, node.end_byte()))
        }
        _ => {
            // `super(...)` and `yield(...)` have node types of their own upstream, so `on_send`
            // never sees them.
            if node
                .child_by_field_name("method")
                .is_none_or(|method| method.kind() == "super")
            {
                return None;
            }
            let list = node.child_by_field_name("arguments")?;
            if list.child(0).map(|child| child.kind()) != Some("(") {
                return None;
            }
            Some((nodes::children(list), list.end_byte()))
        }
    }
}

fn is_assignment_target(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind() == "assignment"
            && parent
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == node.id())
    })
}
