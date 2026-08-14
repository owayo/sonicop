//! `Style/ComparableBetween`: two comparisons around one value are `between?`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Which end of the range a comparison pins down.
#[derive(Clone, Copy)]
enum Bound {
    /// `value >= min` and `min <= value`.
    Min,
    /// `value <= max` and `max >= value`.
    Max,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("binary") {
        if !matches!(
            node.field("operator")
                .map(|op| context.source.node_text(op)),
            Some("&&" | "and")
        ) {
            continue;
        }
        let (Some(left), Some(right)) = (node.field("left"), node.field("right")) else {
            continue;
        };
        // The two matchers differ only in which end is written first. `_value` unifies across
        // both terms, and each term's operator picks exactly one of its alternatives, so at most
        // one of the two ever matches.
        // The second matcher reads the same two terms the other way round: the maximum is named
        // first and the minimum second, which is the same call with the terms swapped.
        let Some((value, min, max)) =
            between(left, right, context).or_else(|| between(right, left, context))
        else {
            continue;
        };
        let replacement = format!(
            "{}.between?({}, {})",
            context.source.node_text(value),
            context.source.node_text(min),
            context.source.node_text(max),
        );
        offenses.push(
            context
                .offense(
                    format!("Prefer `{replacement}` over logical comparison."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The value, the minimum and the maximum, when `lower` pins the low end and `upper` the high one.
fn between<'tree>(
    lower: Node<'tree>,
    upper: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    let (value, min) = comparison(lower, Bound::Min, context)?;
    let (other, max) = comparison(upper, Bound::Max, context)?;
    // `_value` is one wildcard for both terms, so the two have to be the same expression.
    super::nodes::same_tree(context, value, other).then_some((value, min, max))
}

/// One term, answering with the value and the bound it names.
fn comparison<'tree>(
    node: Node<'tree>,
    bound: Bound,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if node.kind_str() != "binary" {
        return None;
    }
    let (left, right) = (node.field("left")?, node.field("right")?);
    let operator = context.source.node_text(node.field("operator")?);
    match (bound, operator) {
        (Bound::Min, ">=") | (Bound::Max, "<=") => Some((left, right)),
        (Bound::Min, "<=") | (Bound::Max, ">=") => Some((right, left)),
        _ => None,
    }
}
