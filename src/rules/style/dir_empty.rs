//! `Style/DirEmpty`: counting a directory's entries is `Dir.empty?`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

/// `minimum_target_ruby_version 2.4`: `Dir.empty?` arrived in 2.4.
const MINIMUM: RubyVersion = RubyVersion::new(2, 4);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of_any(&["binary", "call"]) {
        // `node.block_literal?`: `Dir.each_child(path).none? { ... }` asks something else.
        if node.field("block").is_some() {
            continue;
        }
        let Some((constant, argument, negated)) = matched(node, context) else {
            continue;
        };
        let replacement = format!(
            "{}{}.empty?({})",
            if negated { "!" } else { "" },
            context.source.node_text(constant),
            context.source.slice(argument),
        );
        offenses.push(
            context
                .offense(format!("Use `{replacement}` instead."), node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The four shapes upstream's `offensive?` matches, answering with the `Dir` constant, the
/// directory argument, and whether the check reads as "not empty".
fn matched<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Range<usize>, bool)> {
    match node.kind_str() {
        // `Dir.entries(path).size {== != >} 2` and `Dir.children(path).size {== != >} 0`.
        "binary" => {
            let operator = context.source.node_text(node.field("operator")?);
            if !matches!(operator, "==" | "!=" | ">") {
                return None;
            }
            let right = node.field("right")?;
            if right.kind_str() != "integer" {
                return None;
            }
            let count = context.source.node_text(right);
            let size = call_of(node.field("left")?, "size", context)?;
            if !arguments(size).is_empty() {
                return None;
            }
            let receiver = size.field("receiver")?;
            let listing = call_of(receiver, "entries", context)
                .filter(|_| count == "2")
                .or_else(|| call_of(receiver, "children", context).filter(|_| count == "0"))?;
            let (constant, argument) = dir_call(listing, context)?;
            Some((constant, argument, matches!(operator, "!=" | ">")))
        }
        // `Dir.children(path).empty?` and `Dir.each_child(path).none?`.
        "call" => {
            let selector = context.source.node_text(node.field("method")?);
            let receiver = node.field("receiver")?;
            let listing = match selector {
                "empty?" => call_of(receiver, "children", context),
                "none?" => call_of(receiver, "each_child", context),
                _ => None,
            }?;
            if !is_plain_send(node, context) || !arguments(node).is_empty() {
                return None;
            }
            let (constant, argument) = dir_call(listing, context)?;
            Some((constant, argument, false))
        }
        _ => None,
    }
}

/// The node as a plain `send` of `name`, whatever its receiver.
fn call_of<'tree>(node: Node<'tree>, name: &str, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "call" || node.field("block").is_some() || !is_plain_send(node, context) {
        return None;
    }
    let selector = node.field("method")?;
    (context.source.node_text(selector) == name).then_some(node)
}

/// `(send $(const {nil? cbase} :Dir) _ $_)`: the constant and the one argument the listing takes.
fn dir_call<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Range<usize>)> {
    let receiver = node.field("receiver")?;
    if !super::nodes::is_top_level_constant(receiver, "Dir", context) {
        return None;
    }
    let list = arguments(node);
    let [argument] = list.as_slice() else {
        return None;
    };
    Some((receiver, argument.range()))
}
