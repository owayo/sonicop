//! `Style/EmptyStringInsideInterpolation`: an interpolation that yields `''` says nothing.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_TRAILING_CONDITIONAL: &str = "Do not use trailing conditionals in string interpolation.";
const MSG_TERNARY: &str = "Do not return empty strings in string interpolation.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ternary_style = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "ternary");
    for node in context.nodes_of("interpolation") {
        for child in super::nodes::children_in(node, context) {
            if ternary_style {
                ternary_correction(context, node, child, offenses);
            } else {
                trailing_conditional_correction(context, child, offenses);
            }
        }
    }
}

/// The default style: a conditional written out in full whose one branch is empty becomes a
/// trailing `if` or `unless`.
fn trailing_conditional_correction(
    context: &RuleContext<'_>,
    node: Node<'_>,
    offenses: &mut Vec<Offense>,
) {
    // `child_node.modifier_form?`: what the style asks for is already there.
    let Some((condition, if_branch, else_branch)) = branches(node) else {
        return;
    };
    // Both tests run, but `add_offense` is keyed on the range, so a conditional whose branches are
    // both empty is still reported once -- as the first of the two.
    let replacement = if is_empty(if_branch, context) {
        else_branch.map(|branch| (branch, "unless"))
    } else {
        None
    }
    .or_else(|| match (is_empty(else_branch, context), if_branch) {
        (true, Some(branch)) => Some((branch, "if")),
        _ => None,
    });
    let Some((outcome, keyword)) = replacement else {
        return;
    };
    offenses.push(
        context
            .offense(MSG_TERNARY, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!(
                    "{} {keyword} {}",
                    context.source.node_text(outcome),
                    context.source.node_text(condition)
                ),
                safe: true,
            }),
    );
}

/// The `ternary` style: a trailing conditional becomes a ternary that spells out the empty string.
fn ternary_correction(
    context: &RuleContext<'_>,
    interpolation: Node<'_>,
    node: Node<'_>,
    offenses: &mut Vec<Offense>,
) {
    let unless_form = match node.kind_str() {
        "if_modifier" => false,
        "unless_modifier" => true,
        _ => return,
    };
    let (Some(condition), Some(body)) = (node.field("condition"), node.field("body")) else {
        return;
    };
    let body = context.source.node_text(body);
    let component = if unless_form {
        format!("'' : {body}")
    } else {
        format!("{body} : ''")
    };
    offenses.push(
        context
            .offense(MSG_TRAILING_CONDITIONAL, interpolation.byte_range())
            .corrected_by(Edit {
                start: interpolation.start_byte(),
                end: interpolation.end_byte(),
                replacement: format!("#{{{} ? {component}}}", context.source.node_text(condition)),
                safe: true,
            }),
    );
}

/// The condition and the two branches as upstream's `IfNode` hands them out.
///
/// `if_branch` is the body as it was *written*, so an `unless` keeps its own body there rather than
/// the `else` -- which is why the cops that want the semantic branches swap them for themselves.
fn branches<'tree>(
    node: Node<'tree>,
) -> Option<(Node<'tree>, Option<Node<'tree>>, Option<Node<'tree>>)> {
    if !matches!(node.kind_str(), "if" | "unless" | "conditional") {
        return None;
    }
    Some((
        node.field("condition")?,
        clause(node.field("consequence")),
        clause(node.field("alternative")),
    ))
}

/// The one statement a branch holds.
fn clause<'tree>(clause: Option<Node<'tree>>) -> Option<Node<'tree>> {
    let clause = clause?;
    match clause.kind_str() {
        "then" | "else" => match super::nodes::children(clause).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        _ => Some(clause),
    }
}

/// `empty_branch_outcome?`: `nil` or a string literal holding nothing.
fn is_empty(node: Option<Node<'_>>, context: &RuleContext<'_>) -> bool {
    let Some(node) = node else {
        return false;
    };
    match node.kind_str() {
        "nil" => true,
        "string" => super::literal::node_value(context, node)
            .is_some_and(|decoded| decoded.value.is_empty()),
        _ => false,
    }
}
