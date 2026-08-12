use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;

use super::node_equality::identical;

/// `RESTRICT_ON_SEND`, plus the two keywords that reach `on_and`/`on_or`. Arithmetic is left out on
/// purpose: `x + x` is not a mistake.
const OPERATORS: [&str; 15] = [
    "==", "!=", "===", "<=>", "=~", "&&", "||", ">", ">=", "<", "<=", "|", "^", "and", "or",
];

/// The subset a call can spell. `and` and `or` are keywords rather than method names, so
/// `scope.or(other)` is an ordinary call and no comparison at all.
const OPERATOR_METHODS: [&str; 11] = [
    "==", "!=", "===", "<=>", "=~", ">", ">=", "<", "<=", "|", "^",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((operator, left, right)) = operands(node, context) else {
            continue;
        };
        if !identical(left, right, context) {
            continue;
        }
        offenses.push(context.offense(
            format!("Binary operator `{operator}` has identical operands."),
            node.byte_range(),
        ));
    }
}

/// The operator and the two operands, for both spellings of the same `send`: `a == a` and its
/// dotted form `a.==(a)`, which `binary_operation?` accepts just as readily.
fn operands<'a, 'tree>(
    node: Node<'tree>,
    context: &'a RuleContext<'_>,
) -> Option<(&'a str, Node<'tree>, Node<'tree>)> {
    if node.kind() == "binary" {
        let operator = context.source.node_text(node.child_by_field_name("operator")?);
        return OPERATORS.contains(&operator).then_some((
            operator,
            node.child_by_field_name("left")?,
            node.child_by_field_name("right")?,
        ));
    }
    let receiver = node.child_by_field_name("receiver")?;
    let method = node.child_by_field_name("method")?;
    let operator = context.source.node_text(method);
    if method.kind() != "operator" || !OPERATOR_METHODS.contains(&operator) {
        return None;
    }
    // `node.first_argument`: the rest of the arguments are no part of the comparison.
    let first = arguments(node).first()?.first();
    Some((operator, receiver, first))
}

