//! `Style/HashFetchChain`: fetching a key with a `nil` default and fetching again is `dig`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments};

/// `minimum_target_ruby_version 2.3`: `Hash#dig` arrived in 2.3.
const MINIMUM: RubyVersion = RubyVersion::new(2, 3);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    // `ignore_node`: a call already folded into the chain above it is not reported again.
    let mut ignored: HashSet<usize> = HashSet::new();
    for node in context.nodes_of("call") {
        if ignored.contains(&node.id())
            || node
                .field("method")
                .is_none_or(|name| context.source.node_text(name) != "fetch")
        {
            continue;
        }
        // `last_fetch_non_nil?`: only a chain whose outermost default is `nil` returns what `dig`
        // would.
        if arguments(node)
            .last()
            .is_none_or(|last| last.first().kind_str() != "nil")
        {
            continue;
        }
        let mut keys: Vec<Range<usize>> = Vec::new();
        let mut innermost = None;
        let mut current = Some(node);
        while let Some(call) = current {
            let Some(key) = diggable(call, context) else {
                break;
            };
            keys.insert(0, key);
            ignored.insert(call.id());
            innermost = Some(call);
            current = call.field("receiver");
        }
        let Some(innermost) = innermost else {
            continue;
        };
        if keys.len() < 2 {
            continue;
        }
        // `node.loc.end`: the range runs to the closing parenthesis of the outermost call, which a
        // call written without parentheses does not have.
        let (Some(selector), Some(close)) = (innermost.field("method"), closing_paren(node)) else {
            continue;
        };
        let range = selector.start_byte()..close;
        let replacement = format!(
            "dig({})",
            keys.iter()
                .map(|key| context.source.slice(key.clone()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        offenses.push(
            context
                .offense(format!("Use `{replacement}` instead."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `(call _ :fetch $_arg {nil (hash) (send (const {nil? cbase} :Hash) :new)})`: the key, when the
/// default says "nothing was there".
fn diggable(node: Node<'_>, context: &RuleContext<'_>) -> Option<Range<usize>> {
    if node.kind_str() != "call" || node.field("block").is_some() {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "fetch" {
        return None;
    }
    let list = arguments(node);
    let [key, default] = list.as_slice() else {
        return None;
    };
    is_empty_default(default, context).then(|| key.range())
}

/// The three defaults that stand for "no value": `nil`, `{}` and `Hash.new`.
fn is_empty_default(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    if argument.parts().len() > 1 {
        return false;
    }
    let node = argument.first();
    match node.kind_str() {
        "nil" => true,
        "hash" => node.named_child_count() == 0,
        "call" => {
            node.field("method")
                .is_some_and(|name| context.source.node_text(name) == "new")
                && node.field("receiver").is_some_and(|receiver| {
                    super::nodes::is_top_level_constant(receiver, "Hash", context)
                })
                && arguments(node).is_empty()
        }
        _ => false,
    }
}

/// The `)` that closes a call's argument list.
fn closing_paren(node: Node<'_>) -> Option<usize> {
    let list = node.field("arguments")?;
    let last = list.child(list.child_count().checked_sub(1)? as u32)?;
    (last.kind_str() == ")").then(|| last.end_byte())
}
