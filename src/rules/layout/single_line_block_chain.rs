//! `Layout/SingleLineBlockChain`: a call chained onto a block that fits on one line.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Put method call on a separate line if chained to a single line block.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(receiver), Some(dot)) = (node.field("receiver"), node.field("operator")) else {
            continue;
        };
        // `receiver&.any_block_type?`: the call has to hang off a block.
        let Some(block) = receiver.field("block") else {
            continue;
        };
        // A block already spread over several lines reads fine.
        let closing_line = block.end_position().row;
        if block.start_position().row < closing_line {
            continue;
        }
        // `selector_range`: a `.()` call has no selector, and the `(` stands in for it.
        let Some(selector) = node.field("method").or_else(|| opening_paren(node)) else {
            continue;
        };
        // `call_method_after_block?`: the dot is still on the block's closing line, and it comes
        // before the selector.
        if dot.start_position().row > closing_line
            || dot.start_position().column >= selector.start_position().column
        {
            continue;
        }
        let range = dot.start_byte()..selector.end_byte();
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.start,
            replacement: "\n".to_owned(),
            safe: true,
        }));
    }
}

/// The `(` of a `.()` call, which upstream reads as `loc.begin`.
fn opening_paren<'tree>(node: tree_sitter::Node<'tree>) -> Option<tree_sitter::Node<'tree>> {
    let list = node.field("arguments")?;
    let first = list.child(0)?;
    (first.kind_str() == "(").then_some(first)
}
