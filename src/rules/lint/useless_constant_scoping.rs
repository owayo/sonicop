use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, string_text, symbol_name};

use super::access_modifier::{bare_access_modifier, send_name};
use crate::rules::send_node::named_children_of;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("assignment") {
        let Some(name) = constant_name(node, context) else {
            continue;
        };
        let Some(parent) = node.parent_of(context) else {
            continue;
        };
        let siblings = named_children_of(parent, context);
        let Some(position) = siblings
            .iter()
            .position(|sibling| sibling.id() == node.id())
        else {
            continue;
        };
        // `after_private_modifier?`: the last bare modifier written above the constant, if any.
        let last_modifier = siblings[..position]
            .iter()
            .filter_map(|sibling| bare_access_modifier(*sibling, context))
            .next_back();
        if last_modifier != Some("private") {
            continue;
        }
        // `private_constantize?`: the scope is not useless when the constant is named to
        // `private_constant` below.
        if siblings[position + 1..].iter().any(|sibling| {
            private_constants(*sibling, context).contains(&name)
        }) {
            continue;
        }
        offenses.push(context.offense(
            "Useless `private` access modifier for constant scope.",
            node.byte_range(),
        ));
    }
}

/// `node.name` for a `casgn`: the last part of the constant being written.
fn constant_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let left = node.field("left")?;
    match left.kind_str() {
        "constant" => Some(context.source.node_text(left)),
        "scope_resolution" => Some(context.source.node_text(left.field("name")?)),
        _ => None,
    }
}

/// `(send nil? :private_constant $...)`, reduced to the names its arguments spell.
fn private_constants<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Vec<&'a str> {
    if node.field("receiver").is_some() || send_name(node, context) != Some("private_constant") {
        return Vec::new();
    }
    arguments(node)
        .iter()
        .filter_map(|argument| {
            let node = argument.first();
            symbol_name(node, context)
                .or_else(|| (node.kind_str() == "string").then(|| string_text(node, context)))
        })
        .collect()
}
