use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `OPERATOR_METHODS`, which are written with a space on either side by convention.
const OPERATOR_METHODS: [&str; 29] = [
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~",
];
/// `COMPARISON_OPERATORS`, the method names ending in `=` that are not setters.
const COMPARISON_METHODS: [&str; 5] = ["==", "===", "!=", "<=", ">="];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some((method, argument)) = grouped_argument(node, context) else {
            continue;
        };
        // `operator_method?` / `setter_method?`: both are written with a space before their
        // argument as a matter of course.
        let name = context.source.node_text(method);
        if OPERATOR_METHODS.contains(&name)
            || (name.ends_with('=') && !COMPARISON_METHODS.contains(&name))
        {
            continue;
        }
        // `spaces_before_left_parenthesis`.
        let start = method.end_byte();
        if start >= argument.start_byte() {
            continue;
        }
        let range = start..argument.start_byte();
        offenses.push(
            context
                .offense(
                    format!(
                        "`{}` interpreted as grouped expression.",
                        context.source.node_text(argument)
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// The method name token and the single parenthesized argument, for a call that carries no
/// parentheses of its own.
///
/// `parenthesized_call?` asks whether the argument opens with a `(` that its own location records,
/// which is exactly what tree-sitter writes as a `parenthesized_statements` argument: the same text
/// after a call that *does* have parentheses is the argument list itself.
fn grouped_argument<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let method = node.field("method")?;
    let list = node.field("arguments")?;
    // `node.parenthesized?`.
    if list
        .child(0)
        .is_some_and(|first| !first.is_named() && context.source.node_text(first) == "(")
    {
        return None;
    }
    let call_arguments = arguments(node);
    let [argument] = call_arguments.as_slice() else {
        return None;
    };
    let [argument] = argument.parts() else {
        return None;
    };
    (argument.kind_str() == "parenthesized_statements").then_some((method, *argument))
}
