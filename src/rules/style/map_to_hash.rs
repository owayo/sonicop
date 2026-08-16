//! `Style/MapToHash`: `to_h` takes the block itself, so mapping first builds an array nobody keeps.

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;
use crate::rules::support::final_pos;

/// `minimum_target_ruby_version 2.6`: `to_h` began taking a block in 2.6.
const MINIMUM: RubyVersion = RubyVersion::new(2, 6);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of("call") {
        if node
            .field("method")
            .is_none_or(|name| context.source.node_text(name) != "to_h")
            || !arguments(node).is_empty()
        {
            continue;
        }
        // `to_h_node.block_literal?`: a block on the conversion is the shape being asked for.
        if node.field("block").is_some() {
            continue;
        }
        let (Some(map), Some(dot), Some(selector)) = (
            node.field("receiver"),
            node.field("operator"),
            node.field("method"),
        ) else {
            continue;
        };
        if !super::map_chain::is_mapping(map, context) {
            continue;
        }
        let Some(map_selector) = map.field("method") else {
            continue;
        };
        let dot_source = context.source.node_text(dot);
        let start = final_pos(context.source.text(), dot.start_byte(), false, false, true, false);
        let mut edits = vec![
            Edit {
                start,
                end: selector.end_byte(),
                replacement: String::new(),
                safe: true,
            },
            Edit {
                start: map_selector.start_byte(),
                end: map_selector.end_byte(),
                replacement: "to_h".to_owned(),
                safe: true,
            },
        ];
        // The mapping's own dot takes on the one the conversion was written with, so a safe
        // navigation stays where it belongs.
        if let Some(map_dot) = map.field("operator") {
            edits.push(Edit {
                start: map_dot.start_byte(),
                end: map_dot.end_byte(),
                replacement: dot_source.to_owned(),
                safe: true,
            });
        }
        // `to_h`'s own block is handed a key and a value rather than one pair, so a parameter that
        // destructured the pair loses its parentheses.
        if let Some(argument) = super::map_chain::destructuring_argument(map) {
            let inner = argument.start_byte() + 1..argument.end_byte() - 1;
            edits.push(Edit {
                start: argument.start_byte(),
                end: argument.end_byte(),
                replacement: context.source.slice(inner).to_owned(),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(
                    format!(
                        "Pass a block to `to_h` instead of calling `{}{dot_source}to_h`.",
                        context.source.node_text(map_selector)
                    ),
                    map_selector.byte_range(),
                )
                .corrected_by_all(edits),
        );
    }
}
