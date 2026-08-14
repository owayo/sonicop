//! `Style/MapJoin`: `join` already calls `to_s` on what it joins.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, symbol_name};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node
            .field("method")
            .is_none_or(|name| context.source.node_text(name) != "join")
        {
            continue;
        }
        let Some(map) = node
            .field("receiver")
            .filter(|map| maps_to_s(*map, context))
        else {
            continue;
        };
        let Some(map_selector) = map.field("method") else {
            continue;
        };
        // With a receiver the mapping is cut off after it; without one there is nothing to cut
        // back to, so the removal runs from the mapping to the end of `join`'s own dot.
        let range = match map.field("receiver") {
            Some(receiver) => {
                let Some(dot) = map.field("operator") else {
                    continue;
                };
                // A dot written on the line below its receiver takes the line break with it.
                let start = if receiver.end_position().row < dot.start_position().row {
                    receiver.end_byte()
                } else {
                    dot.start_byte()
                };
                start..map.end_byte()
            }
            None => {
                let Some(dot) = node.field("operator") else {
                    continue;
                };
                map.start_byte()..dot.end_byte()
            }
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Remove redundant `{}(&:to_s)` before `join`.",
                        context.source.node_text(map_selector)
                    ),
                    map_selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// The four spellings of "map every element to its string".
fn maps_to_s(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call"
        || node
            .field("method")
            .is_none_or(|name| !matches!(context.source.node_text(name), "map" | "collect"))
    {
        return false;
    }
    let list = arguments(node);
    let Some(block) = node.field("block") else {
        // `(call _ {:map :collect} (block_pass (sym :to_s)))`.
        return match list.as_slice() {
            [only] => {
                let argument = only.first();
                argument.kind_str() == "block_argument"
                    && super::nodes::children(argument)
                        .first()
                        .is_some_and(|inner| symbol_name(*inner, context) == Some("to_s"))
            }
            _ => false,
        };
    };
    if !list.is_empty() {
        return false;
    }
    let Some(body) = block.field("body") else {
        return false;
    };
    let body = super::nodes::children(body);
    let [statement] = body.as_slice() else {
        return false;
    };
    let Some(subject) = to_s_call(*statement, context) else {
        return false;
    };
    match block.field("parameters") {
        Some(parameters) => match super::nodes::children(parameters).as_slice() {
            [parameter] => {
                parameter.kind_str() == "identifier"
                    && context.source.node_text(*parameter) == context.source.node_text(subject)
            }
            _ => false,
        },
        // A block with no parameters names its argument `_1` or, from 3.4, `it`.
        None => match context.source.node_text(subject) {
            "_1" => true,
            "it" => context.target_ruby_version() >= RubyVersion::new(3, 4),
            _ => false,
        },
    }
}

/// `(send (lvar _x) :to_s)`: the variable a `to_s` was asked of.
fn to_s_call<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "call" || node.field("block").is_some() || !arguments(node).is_empty() {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "to_s" {
        return None;
    }
    let receiver = node.field("receiver")?;
    (receiver.kind_str() == "identifier").then_some(receiver)
}
