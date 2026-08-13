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

/// `Node#recursive_basic_literal?`, which upstream also spells `recursive_literal?`: the two differ
/// only in the branch reached for a type outside `LITERAL_RECURSIVE_TYPES`, and every type that
/// separates `literal?` from `basic_literal?` is inside it, so the two predicates agree everywhere.
///
/// Reachable from `style` too: `Style/YodaCondition` asks the same question of a comparison's two
/// operands.
pub(crate) fn recursive_basic_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind() {
        kind if BASIC.contains(&kind) => true,
        // `emit_file_line_as_literals`: the parser resolves these before a cop sees them, so what
        // reaches one is the `str` or the `int` they stood for rather than the keyword.
        "identifier" => matches!(context.source.node_text(node), "__FILE__" | "__LINE__"),
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
///
/// `__ENCODING__` is one: the parser resolves the keyword into the constant it names, the same way
/// it resolves `__FILE__` and `__LINE__` into literals, while the grammar leaves all three as bare
/// identifiers.
///
/// Reachable from `style` too, for the cops whose patterns pair a constant with a literal.
pub(crate) fn is_constant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind() {
        "constant" | "scope_resolution" => true,
        "identifier" => context.source.node_text(node) == "__ENCODING__",
        _ => false,
    }
}

/// The name upstream's parser gives a literal node, for the cops whose pattern lists types.
///
/// Only literals answer: a pattern that names `{str dstr sym}` never asks about a node type it did
/// not list, so everything else is `None` rather than a name of its own. The three pairs the
/// grammar spells with one node each -- `str`/`dstr`, `sym`/`dsym`, `irange`/`erange` -- are told
/// apart here, since a cop that lists one of a pair and not the other depends on the difference.
///
/// Reachable from `style` too: telling a `str` from a `dstr` is what several Style cops branch on.
pub(crate) fn literal_type(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    Some(match node.kind() {
        "integer" => "int",
        "float" => "float",
        "rational" => "rational",
        "complex" => "complex",
        "true" => "true",
        "false" => "false",
        "nil" => "nil",
        // `?a` and the words of a `%w` list are one-line strings that cannot interpolate.
        "character" | "bare_string" => "str",
        // Adjacent literals are concatenated by the parser into one `dstr` of their parts.
        "chained_string" => "dstr",
        "string" => string_type(node, context),
        "heredoc_beginning" => heredoc_type(node, context),
        "simple_symbol" | "hash_key_symbol" | "bare_symbol" => "sym",
        "delimited_symbol" => {
            if has_interpolation(node) {
                "dsym"
            } else {
                "sym"
            }
        }
        "subshell" => "xstr",
        "array" | "string_array" | "symbol_array" => "array",
        "hash" => "hash",
        "regex" => "regexp",
        "range" => range_type(node, context),
        // The parser folds a sign written before a numeric literal into the literal itself, so
        // `-1` is an `int` and only `-x` stays a call.
        "unary" => {
            let operator = node.child_by_field_name("operator")?;
            if !matches!(context.source.node_text(operator), "-" | "+") {
                return None;
            }
            let operand = node.child_by_field_name("operand")?;
            match literal_type(operand, context)? {
                kind @ ("int" | "float" | "rational" | "complex") => kind,
                _ => return None,
            }
        }
        // `emit_file_line_as_literals`: the parser resolves these while it parses, and a cop only
        // ever sees the literal they stood for.
        "identifier" => match context.source.node_text(node) {
            "__FILE__" => "str",
            "__LINE__" => "int",
            _ => return None,
        },
        _ => return None,
    })
}

/// `str` or `dstr`. The lexer ends a fragment at every newline the literal *holds*, so a literal
/// written over two lines is a `dstr` of two parts -- unless the newline was escaped, which makes
/// it part of the escape rather than of the text.
fn string_type(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    if has_interpolation(node) {
        return "dstr";
    }
    let mut cursor = node.walk();
    let broken = node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "string_content")
        .any(|child| context.source.node_text(child).contains('\n'));
    if broken { "dstr" } else { "str" }
}

/// A heredoc is a `str` only when its body is exactly one fragment: an empty body is a `dstr` of
/// nothing and a two-line body a `dstr` of two parts.
fn heredoc_type(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    let Some(body) = crate::rules::send_node::heredoc_body(node, context) else {
        return "dstr";
    };
    if has_interpolation(body) {
        return "dstr";
    }
    let mut cursor = body.walk();
    let lines: usize = body
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "heredoc_content")
        .map(|child| context.source.node_text(child).matches('\n').count())
        .sum();
    if lines == 1 { "str" } else { "dstr" }
}

/// `irange` or `erange`, which the grammar spells with one node and an operator.
fn range_type(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    let exclusive = node
        .child_by_field_name("operator")
        .is_some_and(|operator| context.source.node_text(operator) == "...");
    if exclusive { "erange" } else { "irange" }
}

/// `node.literal?`.
pub(super) fn is_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    literal_type(node, context).is_some()
}

/// `FALSEY_LITERALS`.
const FALSEY: &[&str] = &["false", "nil"];

/// `node.truthy_literal?`.
pub(super) fn is_truthy_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    literal_type(node, context).is_some_and(|kind| !FALSEY.contains(&kind))
}

/// `node.falsey_literal?`.
pub(super) fn is_falsey_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    literal_type(node, context).is_some_and(|kind| FALSEY.contains(&kind))
}

/// `COMPOSITE_LITERALS`, by the name upstream's parser gives the node.
const COMPOSITE_LITERAL_TYPES: &[&str] = &[
    "dstr", "xstr", "dsym", "array", "hash", "irange", "erange", "regexp",
];

/// `node.basic_literal?`: a literal that holds no other node.
pub(super) fn is_basic_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    literal_type(node, context).is_some_and(|kind| !COMPOSITE_LITERAL_TYPES.contains(&kind))
}
