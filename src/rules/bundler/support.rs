//! The Gemfile declarations the `Bundler` cops search for.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, is_string};

/// Every call to `name` made without a receiver, in source order.
pub(super) fn declarations<'a, 'tree>(
    context: &'a RuleContext<'tree>,
    name: &'a str,
) -> impl Iterator<Item = Node<'tree>> + 'a {
    context.nodes_of("call").filter(move |node| {
        node.field("receiver").is_none()
            && is_plain_send(*node, context)
            && node
                .field("method")
                .is_some_and(|method| context.source.node_text(method) == name)
    })
}

/// `(send nil? :gem str ...)`: a gem named by a plain string. A gem whose name is built at load
/// time is no declaration this cop can order or compare.
pub(super) fn gem_declarations<'a>(
    context: &'a RuleContext<'_>,
) -> impl Iterator<Item = (Node<'a>, Node<'a>)> + 'a {
    declarations(context, "gem").filter_map(|node| {
        let name = arguments(node).first()?.first();
        is_string(name, context).then_some((node, name))
    })
}
