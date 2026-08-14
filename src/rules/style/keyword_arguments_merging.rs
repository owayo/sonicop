//! `Style/KeywordArgumentsMerging`: keyword arguments take the extra keys directly.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments};

const MSG: &str = "Provide additional arguments directly rather than using `merge`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `(send _ _ ... (hash (kwsplat $(send $_ :merge $...)) ...))`: the double splat has to
        // open the hash the call ends with.
        let list = arguments(node);
        let Some(last) = list.last() else {
            continue;
        };
        let Some(splat) = hash_splat(last) else {
            continue;
        };
        let Some(merge) = merge_call(splat, context) else {
            continue;
        };
        let Some(hash) = merge.field("receiver") else {
            continue;
        };
        let extra = arguments(merge);
        if extra
            .iter()
            .any(|argument| argument.first().kind_str() == "block_argument")
        {
            continue;
        }
        let replacement = format!(
            "**{}, {}",
            context.source.node_text(hash),
            extra
                .iter()
                .map(|argument| written_directly(argument, context))
                .collect::<Vec<_>>()
                .join(", ")
        );
        offenses.push(context.offense(MSG, merge.byte_range()).corrected_by(Edit {
            start: splat.start_byte(),
            end: splat.end_byte(),
            replacement,
            safe: true,
        }));
    }
}

/// The `**x` the trailing hash begins with, whether the hash was written with braces or not.
fn hash_splat<'tree>(argument: &Argument<'tree>) -> Option<Node<'tree>> {
    let first = argument.first();
    match first.kind_str() {
        "hash_splat_argument" => Some(first),
        "hash" => match super::nodes::children(first).first() {
            Some(inner) if inner.kind_str() == "hash_splat_argument" => Some(*inner),
            _ => None,
        },
        _ => None,
    }
}

/// `(send $_ :merge $...)`: the call the double splat spreads.
fn merge_call<'tree>(splat: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let parts = super::nodes::children(splat);
    let [value] = parts.as_slice() else {
        return None;
    };
    if value.kind_str() != "call" || value.field("block").is_some() {
        return None;
    }
    (context.source.node_text(value.field("method")?) == "merge").then_some(*value)
}

/// One argument of the `merge`, written the way it would be as a keyword argument: a hash loses its
/// braces, anything else keeps a double splat.
fn written_directly(argument: &Argument<'_>, context: &RuleContext<'_>) -> String {
    let first = argument.first();
    // A brace-less run of pairs is already what a keyword argument list looks like.
    if argument.parts().len() > 1 || matches!(first.kind_str(), "pair" | "hash_splat_argument") {
        return context.source.slice(argument.range()).to_owned();
    }
    if first.kind_str() == "hash" {
        let inner = first.start_byte() + 1..first.end_byte() - 1;
        return context.source.slice(inner).to_owned();
    }
    format!("**{}", context.source.node_text(first))
}
