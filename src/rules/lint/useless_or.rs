use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

use super::locals::LocalVariables;

/// `TRUTHY_RETURN_VALUE_METHODS`: conversions that can never answer `nil` or `false`.
const TRUTHY_RETURN_VALUE_METHODS: &[&str] = &[
    "to_a", "to_c", "to_d", "to_i", "to_f", "to_h", "to_r", "to_s", "to_sym", "intern", "inspect",
    "hash", "object_id", "__id__",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("binary") {
        let (Some(left), Some(right), Some(operator)) =
            (node.field("left"), node.field("right"), node.child(1))
        else {
            continue;
        };
        if !matches!(context.source.node_text(operator), "||" | "or") {
            continue;
        }
        if is_truthy_return_value_method(left, context, &locals) {
            report(context, offenses, node, left);
        } else if is_truthy_return_value_method(right, context, &locals) {
            // The result of the whole `or` is what the next `||` falls back from, so the offense
            // belongs to the operator standing after it.
            let mut parent = node.parent_of(context);
            if parent.is_some_and(|parent| parent.kind_str() == "parenthesized_statements") {
                parent = parent.and_then(|parent| parent.parent_of(context));
            }
            if let Some(parent) = parent.filter(|parent| is_or(*parent, context)) {
                report(context, offenses, parent, right);
            }
        }
    }
}

fn is_or(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .child(1)
            .is_some_and(|operator| matches!(context.source.node_text(operator), "||" | "or"))
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    or_node: Node<'_>,
    truthy: Node<'_>,
) {
    let (Some(left), Some(right), Some(operator)) = (
        or_node.field("left"),
        or_node.field("right"),
        or_node.child(1),
    ) else {
        return;
    };
    let message = format!(
        "`{}` will never evaluate because `{}` always returns a truthy value.",
        context.source.node_text(right),
        context.source.node_text(truthy),
    );
    let whole = or_node.byte_range();
    offenses.push(
        context
            .offense(message, operator.start_byte()..right.end_byte())
            .corrected_by(Edit {
                start: whole.start,
                end: whole.end,
                replacement: context.source.node_text(left).to_owned(),
                safe: true,
            }),
    );
}

/// `(send _ %TRUTHY_RETURN_VALUE_METHODS)`: the call and nothing wrapped around it -- no arguments,
/// no block, and not the safe-navigation form, which is a `csend` upstream.
fn is_truthy_return_value_method(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    match node.kind_str() {
        "call" => {
            let Some(method) = node.field("method") else {
                return false;
            };
            TRUTHY_RETURN_VALUE_METHODS.contains(&context.source.node_text(method))
                && is_plain_send(node, context)
                && node.field("block").is_none()
                && arguments(node).is_empty()
        }
        // A bare name is a receiverless call unless the parser has seen it assigned.
        "identifier" => {
            TRUTHY_RETURN_VALUE_METHODS.contains(&context.source.node_text(node))
                && !locals.is_lvar(node)
        }
        _ => false,
    }
}
