use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const DEFAULT_MAX: usize = 2;

/// `on_csend`: every `&.` call walks its own ancestors and reports the outermost one once the run of
/// safe navigations above it reaches `Max`. Several calls of one chain arrive at the same node, and
/// upstream's `add_offense` keeps only the first report of a range, so the ranges are deduplicated
/// here too.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max = context.setting::<usize>("Max").unwrap_or(DEFAULT_MAX);
    let message = format!("Avoid safe navigation chains longer than {max} calls.");
    let mut reported: HashSet<Range<usize>> = HashSet::new();
    for node in context.nodes_of("call") {
        if !is_safe_navigation(node, context) {
            continue;
        }
        let chains = safe_navigation_chains(node, context);
        if chains.len() < max {
            continue;
        }
        let Some(last) = chains.last() else {
            continue;
        };
        let range = send_node::send_range(*last, context);
        if reported.insert(range.clone()) {
            offenses.push(context.offense(message.clone(), range));
        }
    }
}

/// `node.each_ancestor` taken while every step is a `csend`.
///
/// The walk is not restricted to receivers: `a&.b(c&.d&.e&.f)` counts the enclosing `a&.b` too,
/// because upstream an argument hangs straight off the call it belongs to.
fn safe_navigation_chains<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Vec<Node<'tree>> {
    let mut chains = Vec::new();
    let mut current = node;
    loop {
        // A block written on the call is a `block` node wrapped around it upstream, so the first
        // ancestor is that node and the run of safe navigations stops there.
        if current.field("block").is_some() {
            break;
        }
        let Some(parent) = enclosing(current) else {
            break;
        };
        if !is_safe_navigation(parent, context) {
            break;
        }
        chains.push(parent);
        current = parent;
    }
    chains
}

/// The node upstream would call the parent. An `argument_list` has no counterpart there, so it is
/// stepped over.
fn enclosing<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    if parent.kind_str() == "argument_list" {
        parent.parent()
    } else {
        Some(parent)
    }
}

/// Whether the node is a `csend` -- a call written with `&.`.
fn is_safe_navigation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call" && !send_node::is_plain_send(node, context)
}
