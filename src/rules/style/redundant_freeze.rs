use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;

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
        let Some(selector) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(selector) != "freeze" {
            continue;
        }
        let Some(dot) = node.child_by_field_name("operator") else {
            continue;
        };
        let Some(receiver) = node.child_by_field_name("receiver") else {
            continue;
        };
        let frozen_strings =
            *frozen_strings.get_or_insert_with(|| frozen_string_literals_enabled(context));
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
    if IMMUTABLE_LITERAL_KINDS.contains(&node.kind()) {
        return true;
    }
    if node.kind() == "unary" {
        // `-1` is one `int` upstream: the parser folds a sign written against a numeric literal into
        // the literal itself.
        return node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
            && node.child_by_field_name("operand").is_some_and(|operand| {
                matches!(operand.kind(), "integer" | "float" | "rational" | "complex")
            });
    }
    if frozen_strings && frozen_string(context, node) {
        return true;
    }
    context.target_ruby_version() >= RubyVersion::new(3, 0)
        && matches!(node.kind(), "regex" | "range")
}

/// `(begin $_ ...)`: what upstream reads out of a parenthesized expression before asking whether it
/// is a literal. Only one level comes off, and only the first statement is taken.
fn strip_parenthesis<'tree>(node: Node<'tree>) -> Node<'tree> {
    if node.kind() != "parenthesized_statements" {
        return node;
    }
    super::nodes::children(node)
        .first()
        .copied()
        .unwrap_or(node)
}

/// Whether the file's magic comments turn string literals frozen, which is what makes `.freeze` on
/// one redundant. The default configuration leaves `StringLiteralsFrozenByDefault` unset, so nothing
/// but a comment can enable it.
fn frozen_string_literals_enabled(context: &RuleContext<'_>) -> bool {
    leading_comment_lines(context)
        .find_map(|line| {
            let comment = MagicComment::parse(line);
            comment
                .frozen_string_literal_specified()
                .then(|| comment.frozen_string_literal_enabled())
        })
        .unwrap_or(false)
}

/// The lines above the first one holding code, which is where Ruby reads magic comments.
fn leading_comment_lines<'a>(context: &'a RuleContext<'a>) -> impl Iterator<Item = &'a str> + 'a {
    let first_code = (1..=context.source.line_count()).find(|line_number| {
        let line = context.source.line(*line_number).trim();
        !line.is_empty() && !line.starts_with('#')
    });
    let end = first_code.unwrap_or(context.source.line_count() + 1);
    (1..end).map(|line_number| context.source.line(line_number))
}

/// `frozen_string_literal?` once the file is known to freeze its literals: which literals the
/// feature covers, which widened in Ruby 3.0 from "every `str` and `dstr`" to "the ones nothing is
/// interpolated into".
fn frozen_string(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let string = match node.kind() {
        "string" | "chained_string" | "character" | "heredoc_beginning" => true,
        // A `%w` word is only ever an array element, and a backtick literal is an `xstr` the feature
        // never covered.
        _ => false,
    };
    if !string {
        return false;
    }
    if context.target_ruby_version() < RubyVersion::new(3, 0) {
        return true;
    }
    !interpolated(context, node)
}

/// Whether anything is interpolated into a string literal, which is what upstream's
/// `each_descendant(:begin, :ivar, :cvar, :gvar)` finds inside a `dstr`.
fn interpolated(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let body = match node.kind() {
        "heredoc_beginning" => match send_node::heredoc_body(node, context) {
            Some(body) => body,
            None => return false,
        },
        _ => node,
    };
    send_node::any_descendant(body, &mut |child| child.kind() == "interpolation")
}

fn operation_produces_immutable_object(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'_>,
) -> bool {
    // `(send _ {:count :length :size} ...)` and its block form, neither of which is parenthesized.
    if size_call(context, locals, node) {
        return true;
    }
    if node.kind() != "parenthesized_statements" {
        return false;
    }
    let children = super::nodes::children(node);
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
        && !matches!(left.kind(), "string" | "array")
}

/// A binary operation's operands and operator, however it was written. `1 + 2` and `1.+(2)` are one
/// `send` upstream.
fn operation<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    match node.kind() {
        "binary" => Some((
            node.child_by_field_name("left")?,
            node.child_by_field_name("operator")?,
            node.child_by_field_name("right")?,
        )),
        "call" => {
            if node.child_by_field_name("block").is_some() {
                return None;
            }
            let receiver = node.child_by_field_name("receiver")?;
            let selector = node.child_by_field_name("method")?;
            if selector.kind() != "operator" {
                return None;
            }
            let arguments = node.child_by_field_name("arguments")?;
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
    match node.kind() {
        "integer" | "float" => true,
        "unary" => {
            node.child_by_field_name("operator")
                .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
                && node
                    .child_by_field_name("operand")
                    .is_some_and(|operand| matches!(operand.kind(), "integer" | "float"))
        }
        _ => false,
    }
}

/// `(send _ {:count :length :size} ...)`, with or without a block hung off it.
fn size_call(context: &RuleContext<'_>, locals: &LocalVariables<'_>, node: Node<'_>) -> bool {
    // A receiverless call is a bare identifier here, and a local variable of the same name is an
    // `lvar` upstream that no `send` pattern matches.
    if node.kind() == "identifier" {
        return SIZE_METHODS.contains(&context.source.node_text(node)) && !locals.is_lvar(node);
    }
    node.kind() == "call"
        && send_node::is_plain_send(node, context)
        && node
            .child_by_field_name("method")
            .is_some_and(|selector| SIZE_METHODS.contains(&context.source.node_text(selector)))
}
