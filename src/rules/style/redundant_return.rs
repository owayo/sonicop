use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_multiple_return_values: bool = context
        .setting("AllowMultipleReturnValues")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        let Some(last) = last_body_statement(body) else {
            continue;
        };
        if last.kind() != "return" {
            continue;
        }
        let arguments = return_arguments(last);
        let multiple_values = arguments.len() > 1 && !braceless_hash(&arguments);
        if allow_multiple_return_values && multiple_values {
            continue;
        }
        let message = if multiple_values {
            "Redundant `return` detected. To return multiple values, use an array."
        } else {
            "Redundant `return` detected."
        };
        offenses.push(
            context
                .offense(
                    message,
                    last.start_byte()..last.start_byte() + "return".len(),
                )
                .corrected_by(redundant_return_edit(
                    context,
                    last,
                    &arguments,
                    multiple_values,
                )),
        );
    }
}

/// The expression a method body evaluates last. A trailing comment is a named
/// node here but absent from RuboCop's AST, so it must not stand in for the
/// final expression and hide the `return` behind it.
fn last_body_statement(body: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
        .last()
}

/// The values a `return` yields. RuboCop reads them off the `return` node
/// itself, where a braceless trailing hash has already been folded into one
/// `hash` argument, while tree-sitter keeps its `pair`s separate.
fn return_arguments<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(list) = node
        .named_child(0)
        .filter(|child| child.kind() == "argument_list")
    else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor).collect()
}

fn braceless_hash(arguments: &[Node<'_>]) -> bool {
    !arguments.is_empty() && arguments.iter().all(|argument| argument.kind() == "pair")
}

/// Mirrors RuboCop's autocorrection: an argument-less `return` becomes `nil`,
/// multiple values gain `[]`, a braceless hash gains `{}`, a leading splat is
/// unwrapped, and the keyword plus its trailing space goes away. Dropping the
/// keyword alone would leave `return a, b` as the syntax error `a, b`.
fn redundant_return_edit(
    context: &RuleContext<'_>,
    node: Node<'_>,
    arguments: &[Node<'_>],
    multiple_values: bool,
) -> Edit {
    let (Some(first), Some(last)) = (arguments.first(), arguments.last()) else {
        return Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: "nil".to_owned(),
            safe: true,
        };
    };
    let wrapper = if multiple_values {
        Some(('[', ']'))
    } else if braceless_hash(arguments) {
        Some(('{', '}'))
    } else {
        None
    };
    let splat = arguments
        .iter()
        .any(|argument| argument.kind() == "splat_argument");

    let text = context.source.node_text(node);
    let keyword_end = node.start_byte() + "return".len();
    let whitespace_end = keyword_end
        + text["return".len()..]
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
    if wrapper.is_none() && !splat {
        return Edit {
            start: node.start_byte(),
            end: whitespace_end,
            replacement: String::new(),
            safe: true,
        };
    }

    // Rebuilt rather than spliced so that text the arguments do not cover -
    // `return(1, 2)`'s parentheses - survives verbatim.
    let mut replacement = String::new();
    replacement.push_str(context.source.slice(whitespace_end..first.start_byte()));
    if let Some((open, _)) = wrapper {
        replacement.push(open);
    }
    let first_text = context.source.node_text(*first);
    replacement.push_str(if splat {
        first_text.strip_prefix('*').unwrap_or(first_text)
    } else {
        first_text
    });
    replacement.push_str(context.source.slice(first.end_byte()..last.end_byte()));
    if let Some((_, close)) = wrapper {
        replacement.push(close);
    }
    replacement.push_str(context.source.slice(last.end_byte()..node.end_byte()));
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        safe: true,
    }
}
