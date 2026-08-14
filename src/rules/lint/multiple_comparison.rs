use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use the `&&` operator to compare multiple values.";

const COMPARISON_METHODS: &[&str] = &["<", ">", "<=", ">="];
/// The operators that make `x >= y & y < z` a pair of comparisons joined by a set operation rather
/// than the chain `x >= y >= z` this cop reports.
const SET_OPERATION_OPERATORS: &[&str] = &["&", "|", "^"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("binary") {
        if !is_comparison(node, context) {
            continue;
        }
        // `(send (send _ {:< :> :<= :>=} $_) {:< :> :<= :>=} _)`: the left operand is itself a
        // comparison, whose right operand is the value compared twice.
        let Some(left) = node.field("left") else {
            continue;
        };
        if !is_comparison(left, context) {
            continue;
        }
        let Some(center) = left.field("right") else {
            continue;
        };
        if is_set_operation(center, context) {
            continue;
        }
        let source = context.source.node_text(center);
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: center.start_byte(),
            end: center.end_byte(),
            replacement: format!("{source} && {source}"),
            safe: true,
        }));
    }
}

fn is_comparison(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| {
                COMPARISON_METHODS.contains(&context.source.node_text(operator))
            })
}

fn is_set_operation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| {
                SET_OPERATION_OPERATORS.contains(&context.source.node_text(operator))
            })
}
