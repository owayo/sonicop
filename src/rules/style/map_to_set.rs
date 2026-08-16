//! `Style/MapToSet`: `to_set` takes the block itself, so mapping first builds an array nobody keeps.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;
use crate::rules::support::final_pos;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node
            .field("method")
            .is_none_or(|name| context.source.node_text(name) != "to_set")
            || !arguments(node).is_empty()
        {
            continue;
        }
        // `to_set_node.block_literal?`: a block on the conversion is the shape being asked for.
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
        // `range_with_surrounding_space(side: :left)`: the conversion goes with the blank in front
        // of it, so a chain written over two lines closes up.
        let start = final_pos(context.source.text(), dot.start_byte(), false, false, true, false);
        offenses.push(
            context
                .offense(
                    format!(
                        "Pass a block to `to_set` instead of calling `{}.to_set`.",
                        context.source.node_text(map_selector)
                    ),
                    map_selector.byte_range(),
                )
                .corrected_by_all([
                    Edit {
                        start,
                        end: selector.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: map_selector.start_byte(),
                        end: map_selector.end_byte(),
                        replacement: "to_set".to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}
