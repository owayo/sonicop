//! `Layout/SpaceAroundMethodCallOperator`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Avoid using spaces around a method call operator.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `node.dot? || node.safe_navigation?`: a call written with `::` is neither, so the space
        // after its operator is nobody's business here.
        let Some(operator) = node
            .child_by_field_name("operator")
            .filter(|operator| matches!(context.source.node_text(*operator), "." | "&."))
        else {
            continue;
        };
        if let Some(receiver) = node.child_by_field_name("receiver") {
            check_space(
                context,
                receiver.end_byte(),
                operator.start_byte(),
                offenses,
            );
        }
        if let Some(selector) = selector(node) {
            check_space(
                context,
                operator.end_byte(),
                selector.start_byte(),
                offenses,
            );
        }
    }
    for node in context.nodes_of("scope_resolution") {
        // A constant path written on the left of an assignment is a `casgn` upstream rather than a
        // `const`, and `on_const` never sees it.
        if is_assignment_target(node) {
            continue;
        }
        let (Some(colons), Some(name)) =
            (child_of_kind(node, "::"), node.child_by_field_name("name"))
        else {
            continue;
        };
        check_space(context, colons.end_byte(), name.start_byte(), offenses);
    }
}

/// `node.loc.selector`, falling back to the opening parenthesis for the `Proc#call` shorthand
/// `foo.()`, which carries no selector at all.
fn selector<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.child_by_field_name("method")
        .filter(|method| !method.byte_range().is_empty())
        .or_else(|| {
            node.child_by_field_name("arguments")
                .filter(|arguments| arguments.kind() == "argument_list")
                .and_then(|arguments| arguments.child(0))
        })
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Whether the constant path is being assigned to, which makes it a `casgn` upstream. Only the
/// outermost path is the target: the scope of `A::B::C = 1` is still a plain constant lookup.
fn is_assignment_target(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        "assignment" | "operator_assignment" => parent.child_by_field_name("left") == Some(node),
        "left_assignment_list" | "rest_assignment" => true,
        _ => false,
    }
}

/// `check_space`: only a run of spaces and tabs counts, so an operator written on its own line is
/// left alone.
fn check_space(context: &RuleContext<'_>, begin: usize, end: usize, offenses: &mut Vec<Offense>) {
    if end <= begin {
        return;
    }
    if !context.source.text()[begin..end]
        .bytes()
        .all(|byte| byte == b' ' || byte == b'\t')
    {
        return;
    }
    offenses.push(context.offense(MSG, begin..end).corrected_by(Edit {
        start: begin,
        end,
        replacement: String::new(),
        safe: true,
    }));
}
