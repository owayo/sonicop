//! The `map`/`collect` call that `Style/MapToHash` and `Style/MapToSet` both look for under a
//! conversion.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `{(any_block (call _ {:map :collect}) ...) (call _ {:map :collect} (block_pass sym))}`: a
/// mapping written with a block, or with a symbol handed to it.
pub(super) fn is_mapping(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    if node
        .field("method")
        .is_none_or(|name| !matches!(context.source.node_text(name), "map" | "collect"))
    {
        return false;
    }
    let list = arguments(node);
    if node.field("block").is_some() {
        return list.is_empty();
    }
    match list.as_slice() {
        [only] => {
            let argument = only.first();
            argument.kind_str() == "block_argument"
                && super::nodes::children_in(argument, context)
                    .first()
                    .is_some_and(|inner| {
                        crate::rules::send_node::symbol_name(*inner, context).is_some()
                    })
        }
        _ => false,
    }
}

/// `(args $(mlhs (arg _)+))`: a block whose one parameter destructures what it was handed.
pub(super) fn destructuring_argument<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parameters = node.field("block")?.field("parameters")?;
    match super::nodes::children(parameters).as_slice() {
        [only] if only.kind_str() == "destructured_parameter" => Some(*only),
        _ => None,
    }
}
