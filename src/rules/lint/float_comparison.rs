use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{Argument, arguments, named_children, send_range};
use crate::rules::node_ext::NodeExt;

const MSG_EQUALITY: &str = "Avoid equality comparisons of floats as they are unreliable.";
const MSG_INEQUALITY: &str = "Avoid inequality comparisons of floats as they are unreliable.";
const MSG_CASE: &str = "Avoid float literal comparisons in case statements as they are unreliable.";

/// `RESTRICT_ON_SEND`.
const EQUALITY_METHODS: [&str; 4] = ["==", "!=", "eql?", "equal?"];
/// `ARITHMETIC_OPERATORS`, whose result is a float as soon as either side is one.
const ARITHMETIC_OPERATORS: [&str; 6] = ["+", "-", "*", "/", "%", "**"];
const FLOAT_RETURNING_METHODS: [&str; 3] = ["to_f", "Float", "fdiv"];
/// `FLOAT_INSTANCE_METHODS`. `@-` is upstream's spelling and matches no method name a parser
/// produces, which leaves it dead there as well.
const FLOAT_INSTANCE_METHODS: [&str; 7] = [
    "@-",
    "abs",
    "magnitude",
    "modulo",
    "next_float",
    "prev_float",
    "quo",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "call", "case"]) {
        if node.kind_str() == "case" {
            case_conditions(node, context, offenses);
            continue;
        }
        let Some((method, left, right)) = comparison(node, context) else {
            continue;
        };
        if literal_safe(left, context) || literal_safe(right, context) {
            continue;
        }
        if !is_float(left, context) && !is_float(right, context) {
            continue;
        }
        let message = if method == "!=" {
            MSG_INEQUALITY
        } else {
            MSG_EQUALITY
        };
        offenses.push(context.offense(message, send_range(node, context)));
    }
}

/// `on_case`: every condition of every `when` branch, taken on its own.
fn case_conditions(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for branch in named_children(node) {
        if branch.kind_str() != "when" {
            continue;
        }
        for pattern in named_children(branch) {
            if pattern.kind_str() != "pattern" {
                continue;
            }
            let Some(condition) = named_children(pattern).into_iter().next() else {
                continue;
            };
            if is_float(condition, context) && !literal_safe(condition, context) {
                offenses.push(context.offense(MSG_CASE, condition.byte_range()));
            }
        }
    }
}

/// The two sides of an equality comparison taking exactly one argument, for both spellings of the
/// same call.
fn comparison<'a, 'tree>(
    node: Node<'tree>,
    context: &'a RuleContext<'_>,
) -> Option<(&'a str, Node<'tree>, Node<'tree>)> {
    if node.kind_str() == "binary" {
        let operator = context
            .source
            .node_text(node.field("operator")?);
        if !matches!(operator, "==" | "!=") {
            return None;
        }
        return Some((
            operator,
            node.field("left")?,
            node.field("right")?,
        ));
    }
    let method = context
        .source
        .node_text(node.field("method")?);
    if !EQUALITY_METHODS.contains(&method) {
        return None;
    }
    let receiver = node.field("receiver")?;
    let argument_list = arguments(node);
    let [argument] = argument_list.as_slice() else {
        return None;
    };
    Some((method, receiver, argument.first()))
}

/// `float?`: whether the expression is known to produce a float.
fn is_float(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "float" => true,
        // The parser folds a sign into the numeric literal it stands before.
        "unary" => signed_numeric(node, context).is_some_and(|operand| operand.kind_str() == "float"),
        "binary" | "call" => float_send(node, context),
        "parenthesized_statements" => named_children(node)
            .into_iter()
            .next()
            .is_some_and(|first| is_float(first, context)),
        _ => false,
    }
}

/// `float_send?`: the arithmetic that keeps a float a float, the conversions that make one, and the
/// methods a float answers with another float.
fn float_send(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (method, receiver, first_argument) = call_parts(node, context);
    let Some(method) = method else {
        return false;
    };
    if ARITHMETIC_OPERATORS.contains(&method) {
        return receiver.is_some_and(|receiver| is_float(receiver, context))
            || first_argument.is_some_and(|argument| is_float(argument, context));
    }
    if FLOAT_RETURNING_METHODS.contains(&method) {
        return true;
    }
    let Some(receiver) = receiver.filter(|receiver| is_float_literal(*receiver, context)) else {
        return false;
    };
    FLOAT_INSTANCE_METHODS.contains(&method)
        || numeric_returning_method(method, receiver, first_argument, context)
}

/// `numeric_returning_method?`: the two families whose result depends on an argument or on the sign
/// of the receiver.
fn numeric_returning_method(
    method: &str,
    receiver: Node<'_>,
    first_argument: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> bool {
    match method {
        "angle" | "arg" | "phase" => context
            .source
            .node_text(receiver)
            .replace('_', "")
            .parse::<f64>()
            .is_ok_and(f64::is_sign_negative),
        "ceil" | "floor" | "round" | "truncate" => first_argument
            .and_then(|argument| integer_value(argument, context))
            .is_some_and(|value| value > 0),
        _ => false,
    }
}

/// `literal_safe?`: a zero or a `nil` compares exactly, whichever side it stands on.
fn literal_safe(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "nil" => true,
        "integer" | "float" | "rational" | "complex" => is_zero(node, context),
        "unary" => signed_numeric(node, context).is_some_and(|operand| is_zero(operand, context)),
        "parenthesized_statements" => named_children(node)
            .into_iter()
            .next()
            .is_some_and(|first| literal_safe(first, context)),
        _ => false,
    }
}

fn is_zero(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let text = context.source.node_text(node).replace('_', "");
    let digits = text.trim_end_matches(['r', 'i']);
    digits.parse::<f64>().map_or_else(
        |_| integer_value(node, context) == Some(0),
        |value| value == 0.0,
    )
}

/// The literal a unary `+`/`-` folds into, when there is one.
fn signed_numeric<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let operator = context
        .source
        .node_text(node.field("operator")?);
    if !matches!(operator, "-" | "+") {
        return None;
    }
    let operand = node.field("operand")?;
    matches!(operand.kind_str(), "integer" | "float" | "rational" | "complex").then_some(operand)
}

/// The value of an integer literal, which `Integer(precision.source)` reads the same way.
fn integer_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<i64> {
    if node.kind_str() != "integer" {
        return None;
    }
    let text: String = context
        .source
        .node_text(node)
        .chars()
        .filter(|character| *character != '_')
        .collect();
    let (radix, digits) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (16, &text[2..]),
        Some("0b") => (2, &text[2..]),
        Some("0o") => (8, &text[2..]),
        Some("0d") => (10, &text[2..]),
        _ if text.len() > 1 && text.starts_with('0') => (8, &text[1..]),
        _ => (10, &text[..]),
    };
    i64::from_str_radix(digits, radix).ok()
}

fn is_float_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "float"
        || (node.kind_str() == "unary"
            && signed_numeric(node, context).is_some_and(|operand| operand.kind_str() == "float"))
}

/// The method name, receiver and first argument of a call written either as an operator or with a
/// dot.
fn call_parts<'a, 'tree>(
    node: Node<'tree>,
    context: &'a RuleContext<'_>,
) -> (Option<&'a str>, Option<Node<'tree>>, Option<Node<'tree>>) {
    if node.kind_str() == "binary" {
        let method = node
            .field("operator")
            .map(|operator| context.source.node_text(operator));
        return (
            method,
            node.field("left"),
            node.field("right"),
        );
    }
    let method = node
        .field("method")
        .map(|method| context.source.node_text(method));
    let first_argument = arguments(node)
        .first()
        .map(Argument::first);
    (method, node.field("receiver"), first_argument)
}
