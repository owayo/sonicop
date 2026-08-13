//! `Style/ArrayJoin`: `Array#*` with a string is `join` spelled cryptically.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::is_string;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Favor `Array#join` over `Array#*`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `(send $array :* $str)`: `*` is an operator here rather than a call with a selector.
    for node in context.nodes_of("binary") {
        let (Some(operator), Some(left), Some(right)) = (
            node.field("operator"),
            node.field("left"),
            node.field("right"),
        ) else {
            continue;
        };
        if context.source.node_text(operator) != "*"
            || !matches!(left.kind_str(), "array" | "string_array" | "symbol_array")
            || !is_string(right, context)
        {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, operator.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!(
                        "{}.join({})",
                        context.source.node_text(left),
                        context.source.node_text(right)
                    ),
                    safe: true,
                })
                .corrections_anchored_at(node.byte_range()),
        );
    }
}
