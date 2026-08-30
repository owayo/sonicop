use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::{push_named_children, walk_named};

use super::variable_force::is_lambda;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut reported: Vec<std::ops::Range<usize>> = Vec::new();
    // `on_lvasgn` and its aliases: every assignment whose value the `begin ... end` supplies.
    // `and_asgn` is not among them upstream.
    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        if !is_handled_assignment(node, context) {
            continue;
        }
        for kwbegin in kwbegin_nodes(node) {
            walk_named(kwbegin, context, &mut |inner| {
                if inner.kind_str() != "return" || return_from_inner_scope(inner, kwbegin, context)
                {
                    return;
                }
                let range = inner.byte_range();
                if reported.contains(&range) {
                    return;
                }
                reported.push(range.clone());
                offenses.push(
                    context.offense("Do not `return` in `begin..end` blocks in assignment contexts.", range),
                );
            });
        }
    }
}

/// The assignments the cop has a handler for.
///
/// A plain `=` reaches it only when it writes a name: `foo.bar = x` and `foo[0] = x` are `send`
/// nodes upstream, and a multiple assignment is a `masgn`, none of which the cop handles. An
/// operator assignment always does reach it -- `or_asgn` and `op_asgn` are aliased -- except for
/// `&&=`, whose `and_asgn` is the one the alias list leaves out.
fn is_handled_assignment(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(left) = node.field("left") else {
        return false;
    };
    if node.kind_str() == "operator_assignment" {
        return node
            .child(1)
            .is_some_and(|operator| context.source.node_text(operator) != "&&=");
    }
    matches!(
        left.kind_str(),
        "identifier"
            | "instance_variable"
            | "class_variable"
            | "global_variable"
            | "constant"
            | "scope_resolution"
    )
}

/// `node.each_node(:kwbegin)`: the node itself and every descendant, which for an assignment means
/// the `begin ... end` written as its value or inside it.
fn kwbegin_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let mut stack = Vec::new();
    push_named_children(node, &mut stack);
    while let Some(current) = stack.pop() {
        if current.kind_str() == "begin" {
            found.push(current);
        }
        push_named_children(current, &mut stack);
    }
    found
}

/// `return_from_inner_scope?`: a `return` written inside a nested method or lambda leaves that
/// scope rather than the assignment's.
fn return_from_inner_scope(node: Node<'_>, kwbegin: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if ancestor.id() == kwbegin.id() {
            return false;
        }
        if matches!(ancestor.kind_str(), "method" | "singleton_method" | "lambda")
            || is_lambda(ancestor, context.source, context.ast_index())
        {
            return true;
        }
        current = ancestor.parent_of(context);
    }
    false
}
