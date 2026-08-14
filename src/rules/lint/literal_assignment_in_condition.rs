use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{has_interpolation, heredoc_body, named_children};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&[
        "if",
        "unless",
        "elsif",
        "while",
        "until",
        "if_modifier",
        "unless_modifier",
        "while_modifier",
        "until_modifier",
        "conditional",
    ]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        traverse(condition, &mut |assignment| {
            let (Some(operator), Some(right)) = (assignment.child(1), assignment.field("right"))
            else {
                return;
            };
            if !all_literals(right, context) || is_parallel_assignment_with_splat(right) {
                return;
            }
            let message = format!(
                "Don't use literal assignment `{}` in conditional, should be `==` or non-literal \
                 operand.",
                context
                    .source
                    .slice(operator.start_byte()..right.end_byte())
            );
            offenses.push(context.offense(message, operator.start_byte()..right.end_byte()));
        });
    }
}

/// `traverse_node`: the condition and everything written inside it, except the body of a block or
/// a method definition, which runs somewhere else.
fn traverse(node: Node<'_>, report: &mut impl FnMut(Node<'_>)) {
    if is_equals_assignment(node) {
        report(node);
    }
    let body = matches!(
        node.kind_str(),
        "block" | "do_block" | "lambda" | "method" | "singleton_method"
    )
    .then(|| node.field("body"))
    .flatten();
    for child in named_children(node) {
        if body.is_some_and(|body| body.id() == child.id()) || child.kind_str() == "comment" {
            continue;
        }
        traverse(child, report);
    }
}

/// `equals_asgn?`: the six assignment types written with a plain `=`. An attribute or index write
/// is a `send` upstream and matches none of them.
fn is_equals_assignment(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node.field("left").is_some_and(|left| {
            matches!(
                left.kind_str(),
                "identifier"
                    | "instance_variable"
                    | "class_variable"
                    | "global_variable"
                    | "constant"
                    | "scope_resolution"
                    | "left_assignment_list"
            )
        })
}

/// `all_literals?`: a value the condition could have been written with `==` instead.
fn all_literals(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        // `dstr` and `xstr` are excluded by name, whatever they hold.
        "chained_string" | "subshell" => false,
        "string" => !has_interpolation(node),
        "heredoc_beginning" => is_single_line_heredoc(node, context),
        "array" | "string_array" | "symbol_array" | "right_assignment_list" => named_children(node)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .all(|value| all_literals(value, context)),
        "hash" => named_children(node)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .all(|pair| {
                [pair.field("key"), pair.field("value")]
                    .into_iter()
                    .flatten()
                    .all(|item| all_literals(item, context))
            }),
        // `literal?`: everything else the parser calls a literal, including a range, a regexp and
        // a symbol that interpolates.
        "integer" | "float" | "complex" | "rational" | "simple_symbol" | "delimited_symbol"
        | "hash_key_symbol" | "character" | "true" | "false" | "nil" | "regex" | "range" => true,
        // The parser folds a leading sign into the literal it stands in front of.
        "unary" => node.field("operand").is_some_and(|operand| {
            matches!(
                operand.kind_str(),
                "integer" | "float" | "complex" | "rational"
            ) && matches!(
                node.child(0)
                    .map(|operator| context.source.node_text(operator)),
                Some("-" | "+")
            )
        }),
        _ => false,
    }
}

/// A heredoc is a `str` only while its body is one line; anything else is a `dstr`.
fn is_single_line_heredoc(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(body) = heredoc_body(node, context) else {
        return false;
    };
    let content: Vec<Node<'_>> = named_children(body)
        .into_iter()
        .filter(|child| child.kind_str() != "heredoc_end")
        .collect();
    match content.as_slice() {
        [only] if only.kind_str() == "heredoc_content" => {
            // The content begins with the line break that ended the opener's line, which belongs
            // to the code above rather than to the body.
            let text = context.source.node_text(*only);
            text.strip_prefix('\n').unwrap_or(text).matches('\n').count() == 1
        }
        _ => false,
    }
}

/// `parallel_assignment_with_splat_operator?`: `x = *y` builds an array upstream, but nothing was
/// written that a comparison could stand in for.
fn is_parallel_assignment_with_splat(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "array" | "right_assignment_list")
        && named_children(node)
            .first()
            .is_some_and(|first| first.kind_str() == "splat_argument")
}
