use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, named_children, send_range, symbol_name};

const COMBINATOR_IN_HASH_MSG: &str = "Use `[...].hash` instead of combining hash values manually.";
const MONUPLE_HASH_MSG: &str =
    "Delegate hash directly without wrapping in an array when only using a single value.";
const REDUNDANT_HASH_MSG: &str = "Calling .hash on elements of a hashed array is redundant.";

/// The operators upstream reads as a hand-rolled combination of hash values.
const COMBINATORS: &[&str] = &["^", "+", "*", "|"];

/// The array literals `(array _)` covers: a percent literal is an `array` upstream too.
const ARRAYS: &[&str] = &["array", "string_array", "symbol_array"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `add_offense` keeps a set of the ranges it has already reported and drops a second offense on
    // any of them, whatever its message. This cop needs that: `[a].hash` nested in another hashed
    // array is both a monuple and a redundant hash, and only the first of the two is reported.
    let mut reported: Vec<Range<usize>> = Vec::new();
    let mut report = |message: &'static str, range: Range<usize>, offenses: &mut Vec<Offense>| {
        if reported.contains(&range) {
            return;
        }
        reported.push(range.clone());
        offenses.push(context.offense(message, range));
    };
    for node in context.nodes_of_any(&["call", "binary", "operator_assignment"]) {
        // `outer_bad_hash_combinator?` then `contained_in_hash_method?`: the outermost combination
        // is the one reported, and only inside a definition of `hash` itself.
        if is_combinator(node, context)
            && !ancestors(node, context).any(|ancestor| is_combinator(ancestor, context))
            && ancestors(node, context).any(|ancestor| defines_hash(ancestor, context))
        {
            report(COMBINATOR_IN_HASH_MSG, range_of(node, context), offenses);
        }
        if is_monuple_hash(node, context) {
            report(MONUPLE_HASH_MSG, range_of(node, context), offenses);
        }
        if is_redundant_hash(node, context) {
            report(REDUNDANT_HASH_MSG, range_of(node, context), offenses);
        }
    }
}

/// The range upstream reports, which is its `send` or `op_asgn` node: a block written after the call
/// belongs to the node wrapped around it there rather than to the call itself.
fn range_of(node: Node<'_>, context: &RuleContext<'_>) -> Range<usize> {
    match node.kind_str() {
        "call" => send_range(node, context),
        _ => node.byte_range(),
    }
}

/// `bad_hash_combinator?`: `({send | op-asgn} _ {:^ | :+ | :* | :|} _)`.
fn is_combinator(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "binary" => operator(node, context).is_some_and(|text| COMBINATORS.contains(&text)),
        "operator_assignment" => operator(node, context)
            .and_then(|text| text.strip_suffix('='))
            .is_some_and(|text| COMBINATORS.contains(&text)),
        // `a.^(b)`, which upstream writes as the same `send` as `a ^ b`.
        "call" => {
            is_plain_send(node, context)
                && arguments(node).len() == 1
                && node
                    .field("method")
                    .is_some_and(|method| COMBINATORS.contains(&context.source.node_text(method)))
        }
        _ => false,
    }
}

/// `monuple_hash?`: `(send (array _) :hash)`.
fn is_monuple_hash(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !is_bare_hash_call(node, context) || !is_plain_send(node, context) {
        return false;
    }
    node.field("receiver").is_some_and(|receiver| {
        ARRAYS.contains(&receiver.kind_str()) && element_count(receiver) == 1
    })
}

/// `redundant_hash?`: `(^^(send array ... :hash) _ :hash)`.
///
/// The pattern puts no condition on the node itself beyond its being a two-child node whose second
/// child is the name `hash`, which of the nodes this cop is handed only a `hash` call with no
/// arguments can be -- a safe navigation one included, since `on_csend` is aliased to `on_send`.
fn is_redundant_hash(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !is_bare_hash_call(node, context) {
        return false;
    }
    // Upstream's parent is the `block` when the call carries one, so the node two steps up there is
    // only one step up here.
    let steps = match node.field("block").is_some() {
        true => 1,
        false => 2,
    };
    let mut grandparent = node;
    for _ in 0..steps {
        match grandparent.parent() {
            Some(parent) => grandparent = parent,
            None => return false,
        }
    }
    is_bare_hash_call(grandparent, context)
        && is_plain_send(grandparent, context)
        && grandparent
            .field("receiver")
            .is_some_and(|receiver| ARRAYS.contains(&receiver.kind_str()))
}

/// A call to `hash` that takes no arguments, which is the only shape either `hash` pattern accepts.
fn is_bare_hash_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && node.field("arguments").is_none()
        && node
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "hash")
}

/// `hash_method_definition?`: `def hash`, `def self.hash` or `define_method(:hash)`, each taking no
/// parameters at all.
fn defines_hash(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        // `({def | defs _} :hash (args) _)`
        "method" | "singleton_method" => {
            node.field("name")
                .is_some_and(|name| context.source.node_text(name) == "hash")
                && takes_no_parameters(node)
        }
        // `(block (send _ {:define_method | :define_singleton_method} (sym :hash)) (args) _)`
        "call" => {
            let Some(block) = node.field("block") else {
                return false;
            };
            let defines = node.field("method").is_some_and(|method| {
                matches!(
                    context.source.node_text(method),
                    "define_method" | "define_singleton_method"
                )
            });
            let named_hash = match arguments(node).as_slice() {
                [only] => symbol_name(only.first(), context) == Some("hash"),
                _ => false,
            };
            defines && named_hash && takes_no_parameters(block)
        }
        _ => false,
    }
}

/// `(args)`: a parameter list that was either left out or written empty.
fn takes_no_parameters(node: Node<'_>) -> bool {
    node.field("parameters")
        .is_none_or(|parameters| named_children(parameters).is_empty())
}

/// How many elements an array literal has, with a run of `key: value` elements standing for the one
/// `hash` upstream folds it into.
fn element_count(node: Node<'_>) -> usize {
    let mut count = 0;
    let mut in_hash = false;
    for child in named_children(node) {
        match child.kind_str() {
            "comment" => {}
            "pair" | "hash_splat_argument" => {
                if !in_hash {
                    count += 1;
                }
                in_hash = true;
            }
            _ => {
                count += 1;
                in_hash = false;
            }
        }
    }
    count
}

/// The operator a node was written with, which the grammar leaves as an anonymous token.
fn operator<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named())
        .map(|child| context.source.node_text(child))
}

/// `node.each_ancestor`.
fn ancestors<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> impl Iterator<Item = Node<'tree>> {
    let mut current = Some(node);
    std::iter::from_fn(move || {
        current = current?.parent_of(context);
        current
    })
}
