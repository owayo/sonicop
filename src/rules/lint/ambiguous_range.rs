use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `require_parentheses_for_method_chain?` is `!cop_config[...]`, so the switch being off is
    // what makes a chain acceptable.
    let chains_need_parentheses: bool = context
        .setting("RequireParenthesesForMethodChains")
        .unwrap_or(false);
    for node in context.nodes_of("range") {
        for boundary in [node.field("begin"), node.field("end")]
            .into_iter()
            .flatten()
        {
            if is_acceptable(boundary, context, chains_need_parentheses) {
                continue;
            }
            let range = boundary.byte_range();
            offenses.push(
                context
                    .offense(
                        "Wrap complex range boundaries with parentheses to avoid ambiguity.",
                        range.clone(),
                    )
                    .corrected_by_all([
                        Edit {
                            start: range.start,
                            end: range.start,
                            replacement: "(".to_owned(),
                            safe: true,
                        },
                        Edit {
                            start: range.end,
                            end: range.end,
                            replacement: ")".to_owned(),
                            safe: true,
                        },
                    ]),
            );
        }
    }
}

/// `acceptable?`: a boundary whose extent nobody can misread.
fn is_acceptable(node: Node<'_>, context: &RuleContext<'_>, chains_need_parentheses: bool) -> bool {
    match node.kind_str() {
        // `begin_type?`: already parenthesized.
        "parenthesized_statements"
        // `literal?`, which covers a rational and an imaginary literal too.
        | "integer" | "float" | "rational" | "complex" | "string" | "chained_string"
        | "character" | "simple_symbol" | "delimited_symbol" | "hash_key_symbol" | "true"
        | "false" | "nil" | "regex" | "array" | "hash" | "range" | "string_array"
        | "symbol_array" | "lambda"
        // `variable?`, `const_type?` and `self_type?`.
        | "identifier" | "instance_variable" | "class_variable" | "global_variable"
        | "constant" | "scope_resolution" | "self" => true,
        // `unary_operation?`: `-x` and `!x` read as one thing.
        "unary" => true,
        // `a[1]` is a `send` of `:[]` upstream -- the one operator method the cop lets through.
        "element_reference" => {
            is_acceptable_call(node.field("object"), chains_need_parentheses)
        }
        "call" => {
            let operator = node.field("method").is_some_and(|method| {
                let name = context.source.node_text(method);
                OPERATOR_METHODS.contains(&name) && name != "[]"
            });
            !operator
                && is_acceptable_call(node.field("receiver"), chains_need_parentheses)
        }
        _ => false,
    }
}

/// `acceptable_call?`, given the call's receiver.
fn is_acceptable_call(receiver: Option<Node<'_>>, chains_need_parentheses: bool) -> bool {
    // `receiver&.basic_literal?`: `1.upto(2)` and `"foo".length` read as part of the range.
    if receiver.is_some_and(is_basic_literal) {
        return false;
    }
    !chains_need_parentheses || receiver.is_none()
}

/// `OPERATOR_METHODS`, as `operator_method?` reads them.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

/// `basic_literal?`: a literal with nothing of its own to evaluate.
fn is_basic_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "integer"
            | "float"
            | "rational"
            | "complex"
            | "character"
            | "simple_symbol"
            | "hash_key_symbol"
            | "true"
            | "false"
            | "nil"
    ) || (matches!(node.kind_str(), "string" | "delimited_symbol" | "regex")
        && !has_interpolation(node))
}

fn has_interpolation(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
}
