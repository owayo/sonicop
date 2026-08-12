//! `Node#recursive_basic_literal?`, which is how a cop asks whether a value is known at parse time.
//!
//! Upstream answers from the node type: a `str`, an `int`, a `sym` and the rest of the basic
//! literals are values on their own, a composite literal is one when everything in it is, and a
//! call is one only when it is an operator over such values. tree-sitter names those types
//! differently -- and sometimes not at all, spelling both a `str` and a `dstr` as `string` -- so
//! the mapping is what the answer rests on.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::send_node::has_interpolation;

/// `BASIC_LITERALS`: a literal whose value is the node itself.
const BASIC: &[&str] = &[
    "integer",
    "float",
    "rational",
    "complex",
    "true",
    "false",
    "nil",
    "character",
    "bare_string",
    "simple_symbol",
    "hash_key_symbol",
    "bare_symbol",
];

/// `COMPOSITE_LITERALS` plus the two operator keywords and the two structural types that
/// `LITERAL_RECURSIVE_TYPES` adds: a literal exactly when everything written inside it is.
const COMPOSITE: &[&str] = &[
    "array",
    "string_array",
    "symbol_array",
    "hash",
    "range",
    "regex",
    "subshell",
    "chained_string",
    "pair",
    "parenthesized_statements",
];

/// `LITERAL_RECURSIVE_METHODS`: the operators that carry a literal receiver over to a literal
/// result.
const RECURSIVE_METHODS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<", "*", "!", "<=>"];

pub(super) fn recursive_basic_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind() {
        kind if BASIC.contains(&kind) => true,
        // A quoted literal interpolates or it does not, and only the plain one is basic -- but a
        // `dstr` and a `dsym` are composite literals, so both answers come out the same here as
        // long as everything interpolated into them is a literal too.
        "string" | "delimited_symbol" => !has_interpolation(node) || all_children(node, context),
        kind if COMPOSITE.contains(&kind) => all_children(node, context),
        // `a && b` and `a || b` are `and`/`or` upstream, which recurse; every other binary operator
        // is a `send`, which recurses only for the ten that keep a literal literal.
        "binary" => {
            let Some(operator) = node.child_by_field_name("operator") else {
                return false;
            };
            let text = context.source.node_text(operator);
            (matches!(text, "&&" | "and" | "||" | "or") || RECURSIVE_METHODS.contains(&text))
                && all_children(node, context)
        }
        // The parser folds a leading sign into the literal it precedes; `!x` stays a `send`.
        "unary" => {
            let Some(operator) = node.child_by_field_name("operator") else {
                return false;
            };
            matches!(context.source.node_text(operator), "-" | "+" | "!")
                && node
                    .child_by_field_name("operand")
                    .is_some_and(|operand| recursive_basic_literal(operand, context))
        }
        "call" => {
            node.child_by_field_name("method")
                .is_some_and(|method| RECURSIVE_METHODS.contains(&context.source.node_text(method)))
                && all_children(node, context)
        }
        _ => false,
    }
}

/// `children.compact.all?(&:recursive_basic_literal?)`.
fn all_children(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
        .all(|child| match child.kind() {
            // The parts a quoted literal is written from are not nodes upstream at all: the text
            // between the delimiters is the value, and only what is interpolated is a child.
            "string_content" | "escape_sequence" | "heredoc_content" => true,
            "interpolation" => all_children(child, context),
            _ => recursive_basic_literal(child, context),
        })
}

/// `node.const_type?`: a constant, however it was reached.
pub(super) fn is_constant(node: Node<'_>) -> bool {
    matches!(node.kind(), "constant" | "scope_resolution")
}
