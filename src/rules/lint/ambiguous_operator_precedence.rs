use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `PRECEDENCE`, tightest first. The keyword forms `and` and `or` are deliberately absent: upstream
/// reads their precedence off `node.operator`, which spells them out and so matches nothing here.
const PRECEDENCE: &[&[&str]] = &[
    &["**"],
    &["*", "/", "%"],
    &["+", "-"],
    &["<<", ">>"],
    &["&"],
    &["|", "^"],
    &["&&"],
    &["||"],
];

const MSG: &str = "Wrap expressions with varying precedence with parentheses to avoid ambiguity.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("binary") {
        let Some(operator) = binary_operator(node, context) else {
            continue;
        };
        let Some(parent) = node.parent_of(context) else {
            continue;
        };
        let Some(parent_operator) = binary_operator(parent, context) else {
            continue;
        };
        // `on_and`: an `and` inside an `or` is reported however either was spelled, because the
        // handler asks about the node type rather than about precedence.
        let reportable = if matches!(operator, "&&" | "and") {
            matches!(parent_operator, "||" | "or")
        } else {
            // `on_send`: everything else is compared by the table, which the keyword forms are not
            // in -- so `1 + 2 and 3` is left alone where `1 + 2 && 3` is not.
            match (precedence(operator), precedence(parent_operator)) {
                (Some(inner), Some(outer)) => outer > inner,
                _ => false,
            }
        };
        if !reportable {
            continue;
        }
        let range = node.byte_range();
        offenses.push(context.offense(MSG, range.clone()).corrected_by_all([
            Edit {
                start: range.start,
                end: range.start,
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: range.end,
                end: range.end,
                replacement: ")".to_owned(),
                safe: true,
            },
        ]));
    }
}

/// The operator of a binary expression, or `None` for a node that is not one. `operator?` accepts
/// the keyword forms too, which is why they are recognised here and only ranked below.
fn binary_operator<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if node.kind_str() != "binary" {
        return None;
    }
    let operator = context.source.node_text(node.child(1)?);
    (matches!(operator, "&&" | "||" | "and" | "or") || precedence(operator).is_some())
        .then_some(operator)
}

fn precedence(operator: &str) -> Option<usize> {
    PRECEDENCE
        .iter()
        .position(|operators| operators.contains(&operator))
}
