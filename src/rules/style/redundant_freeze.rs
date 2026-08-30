use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;

use super::frozen_string::{is_frozen, literals_enabled};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not freeze immutable objects, as freezing them has no effect.";

/// `IMMUTABLE_LITERALS`: `LITERALS - MUTABLE_LITERALS`, as the node kinds that spell them.
///
/// `dsym` is among them, so an interpolated symbol counts too, while `dstr` is not -- a string is
/// only frozen where the file says so.
const IMMUTABLE_LITERAL_KINDS: &[&str] = &[
    "integer",
    "float",
    "rational",
    "complex",
    "simple_symbol",
    "delimited_symbol",
    "true",
    "false",
    "nil",
];

/// `{:+ :- :* :** :/ :% :<<}`: what a numeric literal on the left may be operated on with.
const NUMERIC_LEFT_OPERATORS: &[&str] = &["+", "-", "*", "**", "/", "%", "<<"];

/// `{:+ :- :* :** :/ :%}`: the same without `<<`, which appends rather than computes.
const NUMERIC_RIGHT_OPERATORS: &[&str] = &["+", "-", "*", "**", "/", "%"];

/// `COMPARISON_OPERATORS` without `<=>`, which does not answer a boolean.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", "<", ">"];

/// `{:count :length :size}`: the calls that answer with an integer whatever they are sent to.
const SIZE_METHODS: &[&str] = &["count", "length", "size"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    let mut frozen_strings: Option<bool> = None;
    for node in context.nodes_of("call") {
        if !send_node::is_plain_send(node, context) {
            continue;
        }
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "freeze" {
            continue;
        }
        let Some(dot) = node.field("operator") else {
            continue;
        };
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let frozen_strings =
            *frozen_strings.get_or_insert_with(|| literals_enabled(context));
        if !immutable_literal(context, receiver, frozen_strings)
            && !operation_produces_immutable_object(context, &locals, receiver)
        {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, send_node::send_range(node, context))
                .corrected_by_all([
                    Edit {
                        start: dot.start_byte(),
                        end: dot.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: selector.start_byte(),
                        end: selector.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                ]),
        );
    }
}

fn immutable_literal(context: &RuleContext<'_>, node: Node<'_>, frozen_strings: bool) -> bool {
    let node = strip_parenthesis(node);
    if IMMUTABLE_LITERAL_KINDS.contains(&node.kind_str()) {
        return true;
    }
    if node.kind_str() == "unary" {
        // `-1` is one `int` upstream: the parser folds a sign written against a numeric literal into
        // the literal itself.
        return node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
            && node.field("operand").is_some_and(|operand| {
                matches!(operand.kind_str(), "integer" | "float" | "rational" | "complex")
            });
    }
    if frozen_strings && is_frozen(context, node) {
        return true;
    }
    context.target_ruby_version() >= RubyVersion::new(3, 0)
        && matches!(node.kind_str(), "regex" | "range")
}

/// `(begin $_ ...)`: what upstream reads out of a parenthesized expression before asking whether it
/// is a literal. Only one level comes off, and only the first statement is taken.
fn strip_parenthesis<'tree>(node: Node<'tree>) -> Node<'tree> {
    if node.kind_str() != "parenthesized_statements" {
        return node;
    }
    super::nodes::children(node)
        .first()
        .copied()
        .unwrap_or(node)
}

fn operation_produces_immutable_object(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
) -> bool {
    // `(send _ {:count :length :size} ...)` and its block form, neither of which is parenthesized.
    if size_call(context, locals, node) {
        return true;
    }
    if node.kind_str() != "parenthesized_statements" {
        return false;
    }
    let children = super::nodes::children_in(node, context);
    let [only] = children.as_slice() else {
        return false;
    };
    let Some((left, operator, right)) = operation(*only) else {
        return false;
    };
    let operator = context.source.node_text(operator);
    if COMPARISON_OPERATORS.contains(&operator) {
        return true;
    }
    if NUMERIC_LEFT_OPERATORS.contains(&operator) && numeric_literal(context, left) {
        return true;
    }
    NUMERIC_RIGHT_OPERATORS.contains(&operator)
        && numeric_literal(context, right)
        // `!{(str _) array}`: appending to either of those builds something mutable.
        && !matches!(left.kind_str(), "string" | "array")
}

/// A binary operation's operands and operator, however it was written. `1 + 2` and `1.+(2)` are one
/// `send` upstream.
fn operation<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    match node.kind_str() {
        "binary" => Some((
            node.field("left")?,
            node.field("operator")?,
            node.field("right")?,
        )),
        "call" => {
            if node.field("block").is_some() {
                return None;
            }
            let receiver = node.field("receiver")?;
            let selector = node.field("method")?;
            if selector.kind_str() != "operator" {
                return None;
            }
            let arguments = node.field("arguments")?;
            match super::nodes::children(arguments).as_slice() {
                [only] => Some((receiver, selector, *only)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `{float int}`: a numeric literal, sign and all.
fn numeric_literal(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "integer" | "float" => true,
        "unary" => {
            node.field("operator")
                .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
                && node
                    .field("operand")
                    .is_some_and(|operand| matches!(operand.kind_str(), "integer" | "float"))
        }
        _ => false,
    }
}

/// `(send _ {:count :length :size} ...)`, with or without a block hung off it.
fn size_call(context: &RuleContext<'_>, locals: &LocalVariables<'_, '_>, node: Node<'_>) -> bool {
    // A receiverless call is a bare identifier here, and a local variable of the same name is an
    // `lvar` upstream that no `send` pattern matches.
    if node.kind_str() == "identifier" {
        return SIZE_METHODS.contains(&context.source.node_text(node)) && !locals.is_lvar(node);
    }
    node.kind_str() == "call"
        && send_node::is_plain_send(node, context)
        && node
            .field("method")
            .is_some_and(|selector| SIZE_METHODS.contains(&context.source.node_text(selector)))
}
