use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

use super::node_equality::numeric_value;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `(call ${lvar ivar cvar gvar const (send nil? _)} $_ (int $_))`: the grammar writes an
    // arithmetic call as a `binary`, and `x.+(0)` as a `call`.
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((receiver, operation, number)) = operation_parts(node, context) else {
            continue;
        };
        if !is_useless(&operation, number) || !is_reportable_receiver(receiver) {
            continue;
        }
        let range = node.byte_range();
        offenses.push(
            context
                .offense(
                    "Do not apply inconsequential numeric operations to variables.",
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: context.source.node_text(receiver).to_owned(),
                    safe: true,
                }),
        );
    }
    // `(op-asgn ${lvasgn ivasgn cvasgn gvasgn casgn} $_ (int $_))`.
    for node in context.nodes_of("operator_assignment") {
        let (Some(left), Some(right), Some(operator)) =
            (node.field("left"), node.field("right"), node.child(1))
        else {
            continue;
        };
        if !matches!(
            left.kind_str(),
            "identifier" | "instance_variable" | "class_variable" | "global_variable" | "constant"
        ) {
            continue;
        }
        let operation = context
            .source
            .node_text(operator)
            .trim_end_matches('=')
            .to_owned();
        let Some(number) = integer_literal(right, context) else {
            continue;
        };
        if !is_useless(&operation, number) {
            continue;
        }
        let name = context.source.node_text(left);
        let range = node.byte_range();
        offenses.push(
            context
                .offense(
                    "Do not apply inconsequential numeric operations to variables.",
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: format!("{name} = {name}"),
                    safe: true,
                }),
        );
    }
}

/// The receiver, the operator and the integer on the right of an arithmetic call.
fn operation_parts<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, String, f64)> {
    match node.kind_str() {
        "binary" => {
            let operation = context.source.node_text(node.child(1)?).to_owned();
            let number = integer_literal(node.field("right")?, context)?;
            Some((node.field("left")?, operation, number))
        }
        "call" => {
            let operation = context.source.node_text(node.field("method")?).to_owned();
            let call_arguments = arguments(node);
            let [only] = call_arguments.as_slice() else {
                return None;
            };
            let number = integer_literal(only.first(), context)?;
            Some((node.field("receiver")?, operation, number))
        }
        _ => None,
    }
}

/// `(int $_)`: the value of an integer literal, whichever base it was written in and with a
/// leading sign folded in as upstream's parser folds one. A float is a different node type there
/// and never reaches the comparison.
fn integer_literal(node: Node<'_>, context: &RuleContext<'_>) -> Option<f64> {
    let integer = match node.kind_str() {
        "integer" => true,
        "unary" => node
            .field("operand")
            .is_some_and(|operand| operand.kind_str() == "integer"),
        _ => false,
    };
    integer.then(|| numeric_value(node, context)).flatten()
}

/// `{lvar ivar cvar gvar const (send nil? _)}`.
///
/// A bare name is one of the first and last of those whichever way the parser read it, so nothing
/// here has to tell a local variable from a call.
fn is_reportable_receiver(node: Node<'_>) -> bool {
    match node.kind_str() {
        "identifier"
        | "instance_variable"
        | "class_variable"
        | "global_variable"
        | "constant"
        | "scope_resolution" => true,
        // `(send nil? _)`: no receiver, and the pattern leaves no room for arguments.
        "call" => node.field("receiver").is_none() && arguments(node).is_empty(),
        _ => false,
    }
}

/// `useless?`: adding or subtracting zero, and multiplying, dividing or raising by one.
fn is_useless(operation: &str, number: f64) -> bool {
    if number == 0.0 {
        return matches!(operation, "+" | "-");
    }
    number == 1.0 && matches!(operation, "*" | "/" | "**")
}
