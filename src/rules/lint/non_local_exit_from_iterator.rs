use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, named_children};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Non-local exit from iterator, without return value. \
                   `next`, `break`, `Array#find`, `Array#any?`, etc. is preferred.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("return") {
        // `return_value?`: a `return` handing back a value leaves the iterator on purpose.
        if !named_children(node).is_empty() {
            continue;
        }
        let Some(keyword) = node.child(0).filter(|child| child.kind_str() == "return") else {
            continue;
        };
        if escapes_an_iterator(node, context) {
            offenses.push(context.offense(MSG, keyword.byte_range()));
        }
    }
}

/// `each_ancestor(:any_block, :any_def)` with its three exits: a scope of its own stops the search,
/// so does a block handed to `define_method`, and a block without arguments merely passes it on.
fn escapes_an_iterator(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut ancestor = node.parent_of(context);
    while let Some(current) = ancestor {
        ancestor = current.parent_of(context);
        match current.kind_str() {
            // `scoped_node?`: `any_def_type?`.
            "method" | "singleton_method" => return false,
            "block" | "do_block" => {}
            _ => continue,
        }
        let send = block_send(current);
        // `scoped_node?`: `node.lambda?`, which asks only for the method name.
        if is_lambda(current, send, context) {
            return false;
        }
        if defines_a_method(send, context) {
            return false;
        }
        if block_argument_list_is_empty(current, context) {
            continue;
        }
        // `chained_send?`: `(call !nil? ...)`.
        return send.is_some_and(|call| call.field("receiver").is_some());
    }
    false
}

/// The call a block was written on, which is upstream's `send_node`. A `->() {}` has no call of its
/// own there: its parser rewrites the arrow into a receiverless `lambda`, so the block hangs off
/// nothing tree-sitter records.
fn block_send<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    block.parent().filter(|parent| parent.kind_str() == "call")
}

fn is_lambda(block: Node<'_>, send: Option<Node<'_>>, context: &RuleContext<'_>) -> bool {
    if block
        .parent_of(context)
        .is_some_and(|parent| parent.kind_str() == "lambda")
    {
        return true;
    }
    send.and_then(|call| call.field("method"))
        .is_some_and(|method| context.source.node_text(method) == "lambda")
}

/// `define_method?`: `(send _ {:define_method :define_singleton_method} _)`, which takes exactly one
/// argument and any receiver at all.
fn defines_a_method(send: Option<Node<'_>>, context: &RuleContext<'_>) -> bool {
    let Some(call) = send else {
        return false;
    };
    call.field("method").is_some_and(|method| {
        matches!(
            context.source.node_text(method),
            "define_method" | "define_singleton_method"
        )
    }) && is_plain_send(call, context)
        && arguments(call).len() == 1
}

/// `node.argument_list.empty?`. A block that names no argument cannot be the one the `return` was
/// meant for, so the search carries on outwards.
fn block_argument_list_is_empty(block: Node<'_>, context: &RuleContext<'_>) -> bool {
    match block.field("parameters") {
        Some(parameters) => named_children(parameters).is_empty(),
        // A `numblock` / `itblock` declares nothing but still has an argument list.
        None => !super::blocks::uses_implicit_parameter(context, block),
    }
}
