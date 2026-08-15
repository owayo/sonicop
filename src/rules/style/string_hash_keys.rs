use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal;
use crate::rules::send_node;

const MSG: &str = "Prefer symbols instead of strings as hash keys.";

/// The calls whose hash argument is an environment rather than a Ruby hash, where a string key is
/// the only thing that works.
const OPEN3_DIRECT: [&str; 6] = [
    "capture2",
    "capture2e",
    "capture3",
    "popen2",
    "popen2e",
    "popen3",
];
const OPEN3_PIPELINE: [&str; 5] = [
    "pipeline",
    "pipeline_r",
    "pipeline_rw",
    "pipeline_start",
    "pipeline_w",
];

/// `(pair (str _) _)`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("pair") {
        let Some(key) = node.field("key") else {
            continue;
        };
        // `(str _)`: a plain string literal. A heredoc is one too, and upstream skips it.
        if key.kind_str() != "string" || send_node::has_interpolation(key) {
            continue;
        }
        // A quoted key written with the `:` separator is a symbol already -- `'a': 1` is `:a`, not
        // the string `'a'`. The grammar writes it with the same node either separator gets, so the
        // separator itself is what tells them apart.
        if is_symbol_key(node, context) {
            continue;
        }
        if receives_environment(node, context) {
            continue;
        }
        let symbol = ruby_literal::inspect_symbol(&ruby_literal::string_value(key, context));
        offenses.push(context.offense(MSG, key.byte_range()).corrected_by(Edit {
            start: key.start_byte(),
            end: key.end_byte(),
            replacement: symbol,
            safe: true,
        }));
    }
}

/// Whether the pair was written with the `:` separator, which makes a quoted key a symbol.
fn is_symbol_key(pair: Node<'_>, context: &RuleContext<'_>) -> bool {
    pair.child(1)
        .is_some_and(|separator| context.source.node_text(separator) == ":")
}

/// `receive_environments_method?`.
///
/// The patterns count levels from the pair, and the hash they climb through has no node of its own
/// when the keywords were written loose in an argument list -- so the pair itself stands in for it.
fn receives_environment(pair: Node<'_>, context: &RuleContext<'_>) -> bool {
    let hash = pair
        .parent()
        .filter(|parent| parent.kind_str() == "hash")
        .unwrap_or(pair);
    let Some(parent) = enclosing(hash) else {
        return false;
    };
    if is_environment_call(parent, context) {
        return true;
    }
    // `^^^`: the hash sits inside an array that is the argument, which is how `Open3.pipeline`
    // takes its commands.
    enclosing(parent).is_some_and(|grandparent| is_pipeline_call(grandparent, context))
}

/// The node upstream would call the parent. An `argument_list` has no counterpart there.
fn enclosing<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    if parent.kind_str() == "argument_list" {
        parent.parent()
    } else {
        Some(parent)
    }
}

fn is_environment_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let Some(selector) = node.field("method") else {
        return false;
    };
    let method = context.source.node_text(selector);
    match node.field("receiver") {
        // `(send _ {:gsub :gsub!} ...)`.
        Some(receiver) => {
            matches!(method, "gsub" | "gsub!")
                || (method == "popen" && send_node::top_level_constant(receiver, "IO", context))
                || (OPEN3_DIRECT.contains(&method)
                    && send_node::top_level_constant(receiver, "Open3", context))
                // `(send {nil? (const {nil? cbase} :Kernel)} {:spawn :system} ...)`.
                || (matches!(method, "spawn" | "system")
                    && send_node::top_level_constant(receiver, "Kernel", context))
        }
        None => matches!(method, "spawn" | "system"),
    }
}

fn is_pipeline_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method")) else {
        return false;
    };
    OPEN3_PIPELINE.contains(&context.source.node_text(selector))
        && send_node::top_level_constant(receiver, "Open3", context)
}
