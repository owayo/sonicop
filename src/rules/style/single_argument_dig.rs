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
    // `dig_chain_enabled?` is `config.cop_enabled?('Style/DigChain')`, which is
    // `for_cop(name).fetch('Enabled')` -- **the value itself, tested for truth**. `Style/DigChain`
    // ships as `Enabled: pending`, and `'pending'` is truthy in Ruby, so upstream counts it as on
    // and leaves the chain to it. Only a literal `false` turns it off.
    //
    // Reading this as a `bool` and defaulting to `false` therefore had it backwards: `pending`
    // fails to parse as a bool, so the default config -- the one almost everyone runs -- took the
    // chain as unhandled and reported it. `style/single_line_methods.rs` reads its neighbour the
    // right way already; this is the same shape written the other way round.
    let chains = context.setting_of::<bool>("Style/DigChain", "Enabled") != Some(false);
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
        if chains
            && (is_dig(context, receiver)
                || node
                    .parent_of(context)
                    .is_some_and(|parent| is_dig(context, parent)))
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
        let nested = std::iter::successors(node.parent_of(context), |current| {
            current.parent_of(context)
        })
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

fn single_dig_argument<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
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
    let arguments = super::nodes::children_in(arguments, context);
    // `(call _ :dig !{hash block_pass}+)`: at least one argument that is neither a hash nor a
    // block pass. The grammar spells a brace-less hash as its own pairs and splats, and an
    // anonymous `**` as a lone `hash_splat_argument` -- all of them one `hash` upstream.
    arguments.iter().any(|argument| {
        !matches!(
            argument.kind_str(),
            "hash" | "pair" | "hash_splat_argument" | "block_argument"
        )
    })
        && arguments
            .iter()
            .all(|argument| !matches!(argument.kind_str(), "hash" | "pair" | "block_argument"))
}
