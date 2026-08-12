use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, send_range};

const MSG: &str = "Use parentheses in the method call to avoid confusion about precedence.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        let Some(list) = call.child_by_field_name("arguments") else {
            continue;
        };
        let call_arguments = arguments(call);
        if call_arguments.is_empty() || is_parenthesized(list, context) {
            continue;
        }
        let Some(selector) = call.child_by_field_name("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        let first = call_arguments[0].first();
        if first.kind() == "conditional" {
            // `node.method?(:[]) || node.assignment_method?`: neither reads as a call whose
            // argument list could be mistaken for the start of the ternary.
            if name == "[]" || is_assignment_method(name) {
                continue;
            }
            let Some(condition) = first.child_by_field_name("condition") else {
                continue;
            };
            if !is_operator_keyword(condition, context) {
                continue;
            }
            offenses.push(context.offense(MSG, call.start_byte()..condition.end_byte()));
        } else if name.ends_with('?') {
            let last = call_arguments[call_arguments.len() - 1].first();
            if is_operator_keyword(last, context) {
                offenses.push(context.offense(MSG, send_range(call, context)));
            }
        }
    }
}

/// Whether the argument list was written with parentheses, which is what upstream reads off
/// `loc.begin`.
fn is_parenthesized(list: Node<'_>, context: &RuleContext<'_>) -> bool {
    list.child(0)
        .is_some_and(|first| context.source.node_text(first) == "(")
}

/// `node.operator_keyword?`: an `and` or an `or` node, however the operator was spelled.
fn is_operator_keyword(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind() == "binary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                matches!(
                    context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                )
            })
}

/// `node.assignment_method?`: a name ending in `=` that is not one of the comparison operators.
fn is_assignment_method(name: &str) -> bool {
    name.ends_with('=') && !matches!(name, "==" | "!=" | "<=" | ">=" | "===")
}
