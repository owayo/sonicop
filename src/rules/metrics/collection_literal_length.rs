use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{named_children, top_level_constant};

const MSG: &str = "Avoid hard coding large quantities of data in code. \
                   Prefer reading the data from an external source.";

/// The literals whose elements upstream counts through `on_array` and `on_hash`. A percent literal
/// is an `array` there, so the two spellings the grammar gives it belong here as well.
const LITERALS: &[&str] = &["array", "string_array", "symbol_array", "hash"];

/// The places a run of `key: value` elements written without braces stands for a `hash` node of its
/// own upstream. Every one of them is an argument list of some kind, plus the array literal.
const HASH_CONTAINERS: &[&str] = &["argument_list", "array", "element_reference"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `cop_config.fetch('Max', Float::INFINITY)`: without a `Max` nothing is ever long enough.
    let Some(threshold) = context.setting::<usize>("Max") else {
        return;
    };
    for node in context.nodes_of_any(LITERALS) {
        if elements(node).len() >= threshold {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
    // The braceless hash upstream builds out of a trailing run of pairs, which `on_hash` reaches
    // even though no `hash` was written.
    for node in context.nodes_of_any(HASH_CONTAINERS) {
        for (range, length) in braceless_hashes(node) {
            if length >= threshold {
                offenses.push(context.offense(MSG, range));
            }
        }
    }
    // `Set[...]`, which reaches upstream as a `send` of `:[]` because this builder does not emit
    // `index` nodes.
    for node in context.nodes_of("element_reference") {
        let Some(object) = node.field("object") else {
            continue;
        };
        if !top_level_constant(object, "Set", context) || is_assignment_target(node) {
            continue;
        }
        let children = named_children(node);
        let indices = children.get(1..).unwrap_or_default();
        if fold_pairs(indices).len() >= threshold {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}

/// The children upstream's node has.
///
/// A literal's elements are its children there as they are here, with two differences: a comment
/// written inside the literal is no part of the tree at all upstream, and a run of `key: value`
/// elements written inside an array is folded into a single `hash` child.
fn elements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let children: Vec<Node<'tree>> = named_children(node)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    match node.kind_str() {
        // A `hash` written with braces owns its pairs directly; nothing is folded.
        "hash" => children,
        _ => fold_pairs(&children),
    }
}

/// The same children with each run of `key: value` elements standing for the one `hash` upstream
/// builds out of it.
fn fold_pairs<'tree>(children: &[Node<'tree>]) -> Vec<Node<'tree>> {
    let mut folded = Vec::with_capacity(children.len());
    let mut in_hash = false;
    for child in children {
        match is_hash_element(*child) {
            true => {
                if !in_hash {
                    folded.push(*child);
                }
                in_hash = true;
            }
            false => {
                folded.push(*child);
                in_hash = false;
            }
        }
    }
    folded
}

/// Every braceless hash `node` holds, as the span it was written over and how many elements it has.
fn braceless_hashes(node: Node<'_>) -> Vec<(Range<usize>, usize)> {
    let mut runs: Vec<(Range<usize>, usize)> = Vec::new();
    let mut open = false;
    for child in named_children(node) {
        if child.kind_str() == "comment" {
            continue;
        }
        if !is_hash_element(child) {
            open = false;
            continue;
        }
        match (open, runs.last_mut()) {
            (true, Some((range, length))) => {
                range.end = child.end_byte();
                *length += 1;
            }
            _ => {
                runs.push((child.byte_range(), 1));
                open = true;
            }
        }
    }
    runs
}

/// Whether the element is one a braceless hash is built out of.
fn is_hash_element(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "pair" | "hash_splat_argument")
}

/// `Set[a] = b` is a call to `:[]=` rather than to `:[]`, and no handler of this cop sees it.
fn is_assignment_target(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| match parent.kind_str() {
        "assignment" => parent.field("left") == Some(node),
        "left_assignment_list" => true,
        _ => false,
    })
}
