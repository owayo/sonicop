use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Argument kinds the cop steps over: what upstream's node pattern rules out as `!splat`, plus the
/// forwarding and keyword shapes `IGNORED_ARGUMENT_TYPES` names.
const IGNORED: &[&str] = &[
    "splat_argument",
    "block_argument",
    "forward_argument",
    "hash",
    "pair",
    "hash_splat_argument",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let chains = context
        .setting_of::<bool>("Style/DigChain", "Enabled")
        .unwrap_or(false);
    let mut reported: Vec<usize> = Vec::new();

    for node in context.nodes_of("call") {
        // `(send _ :dig $!splat)`: a safe navigation call is a `csend`, which the pattern excludes.
        if node
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "&.")
        {
            continue;
        }
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let Some(argument) = single_dig_argument(context, node) else {
            continue;
        };
        if IGNORED.contains(&argument.kind_str()) {
            continue;
        }
        if chains && (is_dig(context, receiver) || node.parent().is_some_and(|parent| is_dig(context, parent)))
        {
            continue;
        }
        let receiver_source = context.source.node_text(receiver);
        let argument_source = context.source.node_text(argument);
        let message = format!(
            "Use `{receiver_source}[{argument_source}]` instead of `{}`.",
            context.source.node_text(node)
        );
        let offense = context.offense(message, node.byte_range());
        // `ignore_node`: only the outermost `dig` of a chain is rewritten.
        let nested = std::iter::successors(node.parent(), |current| current.parent())
            .any(|ancestor| reported.contains(&ancestor.id()));
        reported.push(node.id());
        offenses.push(match nested {
            true => offense,
            false => offense.corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!("{receiver_source}[{argument_source}]"),
                safe: true,
            }),
        });
    }
}

fn single_dig_argument<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<Node<'tree>> {
    if node.field("block").is_some() {
        return None;
    }
    let method = node.field("method")?;
    if context.source.node_text(method) != "dig" {
        return None;
    }
    let arguments = node.field("arguments")?;
    match super::nodes::children(arguments).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `(call _ :dig !{hash block_pass}+)`: a `dig` with at least one ordinary argument.
fn is_dig(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let Some(method) = node.field("method") else {
        return false;
    };
    if context.source.node_text(method) != "dig" {
        return false;
    }
    let Some(arguments) = node.field("arguments") else {
        return false;
    };
    let arguments = super::nodes::children(arguments);
    !arguments.is_empty()
        && arguments
            .iter()
            .all(|argument| !matches!(argument.kind_str(), "hash" | "pair" | "block_argument"))
}
