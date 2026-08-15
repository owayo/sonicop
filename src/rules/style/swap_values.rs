use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `SIMPLE_ASSIGNMENT_TYPES = %i[lvasgn ivasgn cvasgn gvasgn casgn]`: what a plain
/// `name = value` assigns to. `a.b = v` is a `send` upstream and `a[0] = v` is one too, so neither
/// is here.
const SIMPLE_TARGETS: [&str; 6] = [
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
    "constant",
    "scope_resolution",
];

/// The three-statement dance `tmp = x; x = y; y = tmp`, which one parallel assignment replaces.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("assignment") {
        if !is_simple(node) {
            continue;
        }
        // `node.right_siblings.take(2)`: comments are not nodes upstream, so they are stepped over.
        let Some(parent) = node.parent() else {
            continue;
        };
        let statements: Vec<Node<'_>> = super::nodes::children(parent)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect();
        let Some(position) = statements.iter().position(|child| child.id() == node.id()) else {
            continue;
        };
        let (Some(first), Some(second)) = (
            statements.get(position + 1).copied(),
            statements.get(position + 2).copied(),
        ) else {
            continue;
        };
        if !is_simple(first) || !is_simple(second) {
            continue;
        }
        // `lhs(x_assign) == rhs(tmp_assign) && lhs(y_assign) == rhs(x_assign) &&
        //  rhs(y_assign) == lhs(tmp_assign)`.
        if left(first, context) != right(node, context)
            || left(second, context) != right(first, context)
            || right(second, context) != left(node, context)
        {
            continue;
        }
        let (x, y) = (left(first, context), right(first, context));
        let replacement = format!("{x}, {y} = {y}, {x}");
        let (first_line, _) = context.source.line_column(first.start_byte());
        let (second_line, _) = context.source.line_column(second.start_byte());
        // `range_by_whole_lines(...)` **without** the final newline: the three lines collapse into
        // the one parallel assignment.
        let start = context.source.text()[..node.start_byte()]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        let end = context.source.text()[second.end_byte()..]
            .find('\n')
            .map_or(context.source.len(), |offset| second.end_byte() + offset);
        offenses.push(
            context
                .offense(
                    format!(
                        "Replace this and assignments at lines {first_line} and {second_line} \
                         with `{replacement}`."
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start,
                    end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `simple_assignment?`: a plain assignment to a name, and not one of the shorthand forms, which
/// tree-sitter writes as `operator_assignment` instead.
fn is_simple(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|target| SIMPLE_TARGETS.contains(&target.kind_str()))
}

/// `lhs(node)`: the name being assigned to, `::` included for an absolute constant.
fn left<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    node.field("left")
        .map_or("", |target| context.source.node_text(target))
}

/// `rhs(node)`: `node.expression.source`.
fn right<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    node.field("right")
        .map_or("", |value| context.source.node_text(value))
}
