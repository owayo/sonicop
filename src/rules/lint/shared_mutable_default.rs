use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments, pair_key_symbol, top_level_constant};

const MSG: &str = "Do not create a Hash with a mutable default value as the default value can \
                   accidentally be changed.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        if context.source.node_text(method) != "new"
            || !top_level_constant(receiver, "Hash", context)
        {
            continue;
        }
        let arguments = arguments(node);
        let reportable = match arguments.as_slice() {
            // One argument: the shared object itself, unless it is the table's capacity.
            [only] => is_mutable(only, context) && !is_capacity(only, context),
            // Two: the shared object and the capacity written beside it.
            [first, second] => is_mutable(first, context) && is_capacity(second, context),
            _ => false,
        };
        if reportable {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}

/// `{array hash (send (const {nil? cbase} {:Array :Hash}) :new)}`: the default every lookup would
/// hand back the same instance of.
fn is_mutable(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    let parts = argument.parts();
    if parts.len() > 1 {
        // The run of pairs upstream folds into one `hash`.
        return true;
    }
    let node = argument.first();
    match node.kind_str() {
        "array" | "string_array" | "symbol_array" | "hash" | "pair" | "hash_splat_argument" => true,
        "call" => is_container_new(node, context),
        _ => false,
    }
}

fn is_container_new(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
        return false;
    };
    context.source.node_text(method) == "new"
        && (top_level_constant(receiver, "Array", context)
            || top_level_constant(receiver, "Hash", context))
}

/// `(hash (pair (sym :capacity) _))`: a hash holding exactly that one pair, written with braces or
/// without.
fn is_capacity(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    let parts = argument.parts();
    let pairs = match parts {
        [only] if only.kind_str() == "hash" => {
            let mut cursor = only.walk();
            only.named_children(&mut cursor).collect::<Vec<_>>()
        }
        _ => parts.to_vec(),
    };
    match pairs.as_slice() {
        [only] => pair_key_symbol(*only, context) == Some("capacity"),
        _ => false,
    }
}
