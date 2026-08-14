//! `Style/DigChain`: `dig` takes every key at once, so chaining calls of it says the same thing
//! twice.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `ignore_node`: a call already folded into the chain above it is not reported again. The walk
    // reaches the outermost call of a chain first, which is the one that folds the rest.
    let mut ignored: HashSet<usize> = HashSet::new();
    for node in context.nodes_of("call") {
        if ignored.contains(&node.id()) || !is_dig(node, context) {
            continue;
        }
        // `node.loc.dot`: the outermost call has to be written on a receiver.
        if node.field("receiver").is_none() {
            continue;
        }
        let mut collected: Vec<Range<usize>> = arguments(node)
            .iter()
            .map(|argument| argument.range())
            .collect();
        let mut selector = None;
        let mut current = node;
        while let Some(receiver) = current.field("receiver").filter(|r| is_dig(*r, context)) {
            selector = receiver.field("method");
            let mut inner: Vec<Range<usize>> = arguments(receiver)
                .iter()
                .map(|argument| argument.range())
                .collect();
            inner.append(&mut collected);
            collected = inner;
            ignored.insert(receiver.id());
            current = receiver;
        }
        let Some(selector) = selector else {
            continue;
        };
        // `invalid_arguments?`: `...` only forwards what is left, so anything after it is not an
        // argument the chain can be folded around.
        if collected
            .iter()
            .position(|range| context.source.slice(range.clone()) == "...")
            .is_some_and(|index| index < collected.len() - 1)
        {
            continue;
        }
        let range = selector.start_byte()..node.end_byte();
        let keys = collected
            .iter()
            .map(|argument| context.source.slice(argument.clone()))
            .collect::<Vec<_>>()
            .join(", ");
        let replacement = format!("dig({keys})");
        let mut edits = vec![Edit {
            start: range.start,
            end: range.end,
            replacement: replacement.clone(),
            safe: true,
        }];
        // A comment written inside the chain would be swallowed by the replacement, so it is put
        // back above the whole expression.
        let line = context
            .source
            .line_start(context.source.line_column(node.start_byte()).0);
        for comment in context.comment_ranges().iter().rev() {
            if comment.start < line || comment.start >= range.end {
                continue;
            }
            edits.push(Edit {
                start: node.start_byte(),
                end: node.start_byte(),
                replacement: format!("{}\n", context.source.slice(comment.clone())),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(format!("Use `{replacement}` instead of chaining."), range)
                .corrected_by_all(edits),
        );
    }
}

/// `(call _ :dig !{hash block_pass}+)`.
fn is_dig(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" || node.field("block").is_some() {
        return false;
    }
    if node
        .field("method")
        .is_none_or(|name| context.source.node_text(name) != "dig")
    {
        return false;
    }
    let list = arguments(node);
    !list.is_empty() && !list.iter().any(is_hash_or_block_pass)
}

fn is_hash_or_block_pass(argument: &Argument<'_>) -> bool {
    argument.parts().len() > 1
        || matches!(
            argument.first().kind_str(),
            "hash" | "pair" | "hash_splat_argument" | "block_argument"
        )
}
