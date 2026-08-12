use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// `METHODS_RETURNING_SELF`: the in-place methods whose return value is the receiver.
const METHODS_RETURNING_SELF: &[&str] = &[
    "append",
    "clear",
    "collect!",
    "compare_by_identity",
    "concat",
    "delete_if",
    "fill",
    "initialize_copy",
    "insert",
    "keep_if",
    "map!",
    "merge!",
    "prepend",
    "push",
    "rehash",
    "replace",
    "reverse!",
    "rotate!",
    "shuffle!",
    "sort!",
    "sort_by!",
    "transform_keys!",
    "transform_values!",
    "unshift",
    "update",
];

/// The left-hand sides the parser spells as a plain variable assignment.
const VARIABLES: &[&str] = &[
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("assignment") {
        let (Some(left), Some(right), Some(operator)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
            super::conditional::token(node, &["="]),
        ) else {
            continue;
        };
        let Some((receiver, method)) = self_returning_call(context, right) else {
            continue;
        };
        let message = format!(
            "Redundant self assignment detected. Method `{method}` modifies its receiver in place."
        );
        // `on_lvasgn`: `x = x.concat(y)`.
        if VARIABLES.contains(&left.kind()) {
            if receiver.kind() != left.kind()
                || context.source.node_text(receiver) != context.source.node_text(left)
            {
                continue;
            }
            offenses.push(
                context
                    .offense(message, operator.byte_range())
                    .corrected_by(Edit {
                        start: node.start_byte(),
                        end: node.end_byte(),
                        replacement: context.source.node_text(right).to_owned(),
                        safe: true,
                    }),
            );
            continue;
        }
        // `on_send`: `foo.bar = foo.bar.concat(y)`, which is a call to `:bar=` upstream.
        if left.kind() != "call" || !same_reader(context, left, receiver) {
            continue;
        }
        offenses.push(
            context
                .offense(message, operator.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: right.start_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `(call _ #method_returning_self? ...)`: the receiver and name of a call that returns what it
/// was called on.
fn self_returning_call<'a, 'tree>(
    context: &'a RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'a str)> {
    if node.kind() != "call" {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    let name = context.source.node_text(method);
    if !METHODS_RETURNING_SELF.contains(&name) {
        return None;
    }
    Some((node.child_by_field_name("receiver")?, name))
}

/// `(call %1 %2)`: the reader that matches the writer being assigned, taking no arguments of its
/// own.
fn same_reader(context: &RuleContext<'_>, writer: Node<'_>, reader: Node<'_>) -> bool {
    if reader.kind() != "call"
        || reader.child_by_field_name("arguments").is_some()
        || reader.child_by_field_name("block").is_some()
    {
        return false;
    }
    let (Some(written), Some(read)) = (
        writer.child_by_field_name("receiver"),
        reader.child_by_field_name("receiver"),
    ) else {
        return false;
    };
    if context.source.node_text(written) != context.source.node_text(read) {
        return false;
    }
    let (Some(setter), Some(getter)) = (
        writer.child_by_field_name("method"),
        reader.child_by_field_name("method"),
    ) else {
        return false;
    };
    context.source.node_text(setter) == context.source.node_text(getter)
}
