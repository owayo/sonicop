use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Operators RuboCop's `Node#recursive_literal?` looks through
/// (`LITERAL_RECURSIVE_METHODS`): comparisons plus `*`, `!` and `<=>`. Note the
/// absence of `+`, so `"#{1 + 1}"` counts as dynamic while `"#{1 * 2}"` does not.
const LITERAL_RECURSIVE_OPERATORS: &[&str] =
    &["==", "===", "!=", "<=", ">=", ">", "<", "*", "!", "<=>"];

const NUMERIC_LEAF_KINDS: &[&str] = &["integer", "float", "rational", "complex"];

/// Leaf nodes that are literals outright (`BASIC_LITERALS`, plus the fragments
/// tree-sitter splits literal text into).
const LITERAL_LEAF_KINDS: &[&str] = &[
    "integer",
    "float",
    "rational",
    "complex",
    "true",
    "false",
    "nil",
    "simple_symbol",
    "hash_key_symbol",
    "character",
    "string_content",
    "escape_sequence",
    "heredoc_content",
    "heredoc_end",
    "regex_options",
];

/// Literals built out of other nodes (`COMPOSITE_LITERALS`, plus `begin`/`pair`
/// and tree-sitter's own grouping nodes): literal only if every part is.
const LITERAL_COMPOSITE_KINDS: &[&str] = &[
    "string",
    "bare_string",
    "chained_string",
    "delimited_symbol",
    "subshell",
    "regex",
    "array",
    "string_array",
    "symbol_array",
    "hash",
    "pair",
    "range",
    "interpolation",
    "heredoc_body",
    "parenthesized_statements",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "eval" || !receiver_is_eval_scope(node, context) {
            continue;
        }
        let Some(arguments) = node.field("arguments") else {
            continue;
        };
        let Some(argument) = arguments.named_child(0) else {
            continue;
        };
        if literal_code(argument, context) {
            continue;
        }
        offenses.push(context.offense(
            "The use of `eval` is a serious security risk.",
            method.byte_range(),
        ));
    }
}

/// RuboCop only matches `eval`, `binding.eval` and `Kernel.eval` - the receivers
/// that evaluate in the caller's own scope. `::Kernel` is the same constant, but
/// `Binding` and any other constant are different methods entirely.
fn receiver_is_eval_scope(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(receiver) = call.field("receiver") else {
        return true;
    };
    match receiver.kind_str() {
        "constant" => context.source.node_text(receiver) == "Kernel",
        // `::Kernel`, but not a `Foo::Kernel` that merely ends in the name.
        "scope_resolution" => {
            receiver.field("scope").is_none()
                && receiver
                    .field("name")
                    .is_some_and(|name| context.source.node_text(name) == "Kernel")
        }
        "identifier" => context.source.node_text(receiver) == "binding",
        _ => false,
    }
}

/// Whether the evaluated argument is a literal chunk of code. RuboCop exempts a
/// plain string outright, and an interpolated one whose interpolations are all
/// recursively literal, because neither can smuggle in new code. Only those two
/// shapes are exempt: a backtick command or a symbol still counts as an offense
/// even though both are literals.
fn literal_code(argument: Node<'_>, context: &RuleContext<'_>) -> bool {
    match argument.kind_str() {
        "string" | "chained_string" => recursive_literal(argument, context),
        // A heredoc's body is a sibling of the enclosing statement rather than a
        // child of the opener, so it has to be looked up separately.
        "heredoc_beginning" => {
            heredoc_body(argument, context).is_some_and(|body| recursive_literal(body, context))
        }
        _ => false,
    }
}

/// The `heredoc_body` opened by `beginning`. Bodies appear after the statement in
/// the same order as their openers, so the nth opener owns the nth body.
fn heredoc_body<'a>(beginning: Node<'_>, context: &'a RuleContext<'_>) -> Option<Node<'a>> {
    let position = context
        .nodes_of("heredoc_beginning")
        .position(|node| node.id() == beginning.id())?;
    context.nodes_of("heredoc_body").nth(position)
}

/// Mirrors RuboCop's `Node#recursive_literal?`.
fn recursive_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let kind = node.kind_str();
    if LITERAL_LEAF_KINDS.contains(&kind) {
        return true;
    }
    if LITERAL_COMPOSITE_KINDS.contains(&kind) {
        return named_children(node).all(|child| recursive_literal(child, context));
    }
    // A heredoc interpolated into the evaluated string reaches RuboCop as a plain `str`, so it is
    // literal exactly when its own body is. Its body is a sibling of the statement, not a child.
    if kind == "heredoc_beginning" {
        return heredoc_body(node, context).is_some_and(|body| recursive_literal(body, context));
    }
    if matches!(kind, "unary" | "binary" | "boolean") {
        let Some(operator) = node
            .field("operator")
            .map(|operator| context.source.node_text(operator))
            .or_else(|| operator_token(node, context))
        else {
            return false;
        };
        // A signed number is a single numeric literal to RuboCop's parser rather
        // than a call, so `-1` is literal while `-foo` is not.
        if kind == "unary" && matches!(operator, "-" | "+") {
            return node
                .field("operand")
                .is_some_and(|operand| NUMERIC_LEAF_KINDS.contains(&operand.kind_str()));
        }
        return (LITERAL_RECURSIVE_OPERATORS.contains(&operator)
            || matches!(operator, "and" | "or"))
            && named_children(node).all(|child| recursive_literal(child, context));
    }
    false
}

/// The operator of a node that exposes it as an anonymous token rather than a
/// named `operator` field.
fn operator_token<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named())
        .map(|child| context.source.node_text(child))
}

fn named_children<'tree>(node: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .collect::<Vec<_>>()
        .into_iter()
}
