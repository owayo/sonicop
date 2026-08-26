use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Top level return with argument detected.";

/// The scopes a `return` may belong to. Written outside all of them it ends the file, and an
/// argument there is silently dropped.
const SCOPES: &[&str] = &["method", "singleton_method", "block", "do_block", "lambda"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("return") {
        // `return_node.arguments?`: `return()` names none. The grammar keeps the empty parentheses
        // as an `argument_list` all the same, so the list has to be looked into.
        if !has_arguments(node) || !top_level(node) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: "return".to_owned(),
            safe: true,
        }));
    }
}

/// Whether the `return` was given anything to hand back.
fn has_arguments(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|child| {
        child.kind_str() != "argument_list" || child.named_child_count() > 0
    })
}

fn top_level(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if SCOPES.contains(&ancestor.kind_str()) {
            return false;
        }
        current = ancestor.parent();
    }
    true
}
