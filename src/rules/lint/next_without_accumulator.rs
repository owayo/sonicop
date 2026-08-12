use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;

use super::statements::statements;

const MSG: &str = "Use `next` with an accumulator argument in a `reduce`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        let Some(block) = call.child_by_field_name("block") else {
            continue;
        };
        let Some(method) = call.child_by_field_name("method") else {
            continue;
        };
        if !matches!(context.source.node_text(method), "reduce" | "inject") {
            continue;
        }
        // `(call _recv {:reduce :inject} !sym)`: one argument, and not the symbol form that names
        // an operator instead of taking a block.
        let call_arguments = arguments(call);
        let [seed] = call_arguments.as_slice() else {
            continue;
        };
        if matches!(
            seed.first().kind(),
            "simple_symbol" | "delimited_symbol" | "hash_key_symbol" | "bare_symbol"
        ) {
            continue;
        }
        // `$(begin ...)`: the body has to be a sequence, so a block holding one statement never
        // matches however that statement is written.
        let body = block.child_by_field_name("body");
        if body.is_none_or(|body| statements(body).len() < 2) {
            continue;
        }
        let Some(body) = body else { continue };
        if let Some(void) = void_next(body, block) {
            offenses.push(context.offense(MSG, void.byte_range()));
        }
    }
}

/// The first bare `next` that belongs to this block rather than to one written inside it.
fn void_next<'tree>(node: Node<'tree>, block: Node<'_>) -> Option<Node<'tree>> {
    if node.kind() == "next" && node.named_child_count() == 0 && owning_block(node, block) {
        return Some(node);
    }
    let mut cursor = node.walk();
    let children: Vec<Node<'tree>> = node.named_children(&mut cursor).collect();
    children
        .into_iter()
        .find_map(|child| void_next(child, block))
}

/// `node.each_ancestor(:any_block).first == node`.
fn owning_block(node: Node<'_>, block: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if matches!(ancestor.kind(), "block" | "do_block" | "lambda") {
            return ancestor.id() == block.id();
        }
        current = ancestor.parent();
    }
    false
}
