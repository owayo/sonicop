use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

use super::locals::LocalVariables;
use super::node_equality::numeric_value;

const MSG: &str = "Numeric operation with a constant result detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    // `(call (call nil? $_lhs) $_operation ({int | call nil?} $_rhs))`: the two sides are compared
    // by the name or the value written, not by the node.
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((left, operation, right)) = operation_parts(node, context) else {
            continue;
        };
        let (Some(lhs), Some(rhs)) = (
            bare_call_name(left, context, &locals),
            integer_operand(right, context).or_else(|| bare_call_name(right, context, &locals)),
        ) else {
            continue;
        };
        let Some(result) = constant_result(&lhs, &operation, &rhs) else {
            continue;
        };
        let range = node.byte_range();
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: result.to_string(),
            safe: true,
        }));
    }
    // `(op-asgn (lvasgn $_lhs) $_operation ({int lvar} $_rhs))`. The write itself declares the
    // local, so a bare name on the right is one.
    for node in context.nodes_of("operator_assignment") {
        let (Some(left), Some(right), Some(operator)) =
            (node.field("left"), node.field("right"), node.child(1))
        else {
            continue;
        };
        if left.kind_str() != "identifier" {
            continue;
        }
        let lhs = context.source.node_text(left).to_owned();
        let operation = context
            .source
            .node_text(operator)
            .trim_end_matches('=')
            .to_owned();
        let Some(rhs) = integer_operand(right, context).or_else(|| {
            (right.kind_str() == "identifier").then(|| context.source.node_text(right).to_owned())
        }) else {
            continue;
        };
        let Some(result) = constant_result(&lhs, &operation, &rhs) else {
            continue;
        };
        let range = node.byte_range();
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: format!("{lhs} = {result}"),
            safe: true,
        }));
    }
}

/// The left side, the operator and the right side of an arithmetic call.
fn operation_parts<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, String, Node<'tree>)> {
    match node.kind_str() {
        "binary" => Some((
            node.field("left")?,
            context.source.node_text(node.child(1)?).to_owned(),
            node.field("right")?,
        )),
        "call" => {
            // A safe navigation call is a `csend`, which the cop returns on straight away.
            if node
                .field("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "&.")
            {
                return None;
            }
            let call_arguments = arguments(node);
            let [only] = call_arguments.as_slice() else {
                return None;
            };
            Some((
                node.field("receiver")?,
                context.source.node_text(node.field("method")?).to_owned(),
                only.first(),
            ))
        }
        _ => None,
    }
}

/// `(call nil? $_name)`: a receiverless call with no arguments. A name the parser resolved to a
/// local variable is an `lvar` there and matches nothing the pattern lists.
fn bare_call_name(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<String> {
    match node.kind_str() {
        "identifier" if !locals.is_lvar(node) => Some(context.source.node_text(node).to_owned()),
        "call" if node.field("receiver").is_none() && arguments(node).is_empty() => {
            Some(context.source.node_text(node.field("method")?).to_owned())
        }
        _ => None,
    }
}

/// `(int $_)` reduced to what the cop asks of it: whether `to_s` would print `0`.
///
/// The parser folds a leading sign into the literal and reads every base the same way, so `-0` and
/// `0x0` are the same zero as `0`. Any other number is spelled back as something no method name
/// could be, since the other half of the comparison is a name.
fn integer_operand(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let integer = match node.kind_str() {
        "integer" => true,
        "unary" => node
            .field("operand")
            .is_some_and(|operand| operand.kind_str() == "integer"),
        _ => false,
    };
    let value = integer.then(|| numeric_value(node, context)).flatten()?;
    Some(if value == 0.0 { "0" } else { " " }.to_owned())
}

/// `constant_result?`: multiplying by zero, raising to zero, and dividing a value by itself.
fn constant_result(lhs: &str, operation: &str, rhs: &str) -> Option<u8> {
    if rhs == "0" {
        return match operation {
            "*" => Some(0),
            "**" => Some(1),
            _ => None,
        };
    }
    (rhs == lhs && operation == "/").then_some(1)
}
