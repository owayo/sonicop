use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;

/// `OPS`: the binary operators that have a self-assignment shorthand.
const OPS: &[&str] = &["+", "-", "*", "**", "/", "%", "^", "<<", ">>", "|", "&"];

/// The operators upstream's parser spells as an `and` / `or` node rather than a call, which the cop
/// reaches through `operator_keyword?`. Only `&&` and `||` can stand as the right-hand side of an
/// assignment: `x = x and y` binds as `(x = x) and y`.
const KEYWORD_OPS: &[&str] = &["&&", "||", "and", "or"];

/// The left-hand sides upstream hooks: `on_lvasgn`, `on_ivasgn` and `on_cvasgn`. There is no
/// `on_gvasgn`, so `$x = $x + 1` is left alone.
const TARGETS: &[&str] = &["identifier", "instance_variable", "class_variable"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("assignment") {
        let (Some(left), Some(right), Some(operator)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
            super::conditional::token(node, &["="]),
        ) else {
            continue;
        };
        if !TARGETS.contains(&left.kind()) {
            continue;
        }
        let Some((method, receiver, replacement)) = shorthand(right, context) else {
            continue;
        };
        if receiver.kind() != left.kind()
            || context.source.node_text(receiver) != context.source.node_text(left)
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Use self-assignment shorthand `{method}=`."),
                    node.byte_range(),
                )
                // `insert_before(node.loc.operator, ...)` hangs off the `=` rather than off the
                // assignment the offense was reported on.
                .corrections_anchored_at(operator.byte_range())
                .corrected_by_all([
                    Edit {
                        start: operator.start_byte(),
                        end: operator.start_byte(),
                        replacement: method.to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: right.start_byte(),
                        end: right.end_byte(),
                        replacement: context.source.node_text(replacement).to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// The operator the right-hand side applies, what it applies it to, and what would be left once the
/// assignment took the shorthand form.
fn shorthand<'a, 'tree>(
    right: Node<'tree>,
    context: &'a RuleContext<'_>,
) -> Option<(&'a str, Node<'tree>, Node<'tree>)> {
    match right.kind() {
        // `x + 1`, which upstream reads as a call taking one argument, and `x && y`, which it reads
        // as an `and` node. The grammar writes both as a binary expression.
        "binary" => {
            let operator = context
                .source
                .node_text(right.child_by_field_name("operator")?);
            if !OPS.contains(&operator) && !KEYWORD_OPS.contains(&operator) {
                return None;
            }
            Some((
                operator,
                right.child_by_field_name("left")?,
                right.child_by_field_name("right")?,
            ))
        }
        // `x = x.+(1)`, the same call written out.
        "call" => {
            let method = context
                .source
                .node_text(right.child_by_field_name("method")?);
            if !OPS.contains(&method) {
                return None;
            }
            let list = arguments(right);
            let [argument] = list.as_slice() else {
                return None;
            };
            let [only] = argument.parts() else {
                return None;
            };
            Some((method, right.child_by_field_name("receiver")?, *only))
        }
        _ => None,
    }
}
