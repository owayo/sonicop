//! `Style/FetchEnvVar`: reading an environment variable with `[]` hides that it may be missing.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `RuboCop::AST::Node::COMPARISON_OPERATORS`.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<", "<=>"];

/// The node kinds upstream's parser writes as a `send`.
const SEND_KINDS: &[&str] = &["call", "binary", "unary", "element_reference"];

/// The operators upstream's parser gives `and` and `or` nodes to rather than calls.
const LOGICAL_OPERATORS: &[&str] = &["&&", "||", "and", "or"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let default_to_nil = context.setting::<bool>("DefaultToNil").unwrap_or(true);
    let allowed: Vec<String> = context.setting("AllowedVariables").unwrap_or_default();
    for node in context.nodes_of("element_reference") {
        // `(send (const nil? :ENV) :[] $_)`: the scope has to be absent, so `::ENV['X']` is a
        // different constant and never matches.
        let Some(object) = node.field("object").filter(|object| {
            object.kind_str() == "constant" && context.source.node_text(*object) == "ENV"
        }) else {
            continue;
        };
        let _ = object;
        let parts = children(node);
        let [_, key] = parts.as_slice() else {
            continue;
        };
        if is_allowed_variable(*key, &allowed, context) || allowable_use(node, context) {
            continue;
        }
        let key = context.source.node_text(*key);
        let replacement = if default_to_nil {
            format!("ENV.fetch({key}, nil)")
        } else {
            format!("ENV.fetch({key})")
        };
        offenses.push(
            context
                .offense(
                    format!("Use `{replacement}` instead of `ENV[{key}]`."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `allowed_var?`: a key the configuration exempts.
fn is_allowed_variable(key: Node<'_>, allowed: &[String], context: &RuleContext<'_>) -> bool {
    if allowed.is_empty() || !crate::rules::send_node::is_string(key, context) {
        return false;
    }
    let value = crate::rules::send_node::string_text(key, context);
    allowed.iter().any(|entry| entry == value)
}

/// `allowable_use?`: the four readings where `[]` already says what it means.
fn allowable_use(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    used_as_flag(node, context)
        || message_chained_with_dot(node, context)
        || assigned(node, context)
        || or_lhs(node, context)
}

/// `used_as_flag?`.
fn used_as_flag(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = upstream_parent(node, context) else {
        return false;
    };
    if used_if_condition_in_body(node, context) {
        return true;
    }
    // `node.parent.prefix_bang?` and `comparison_method?`.
    if parent.kind_str() == "unary" {
        return parent
            .child(0)
            .is_some_and(|operator| operator.kind_str() == "!");
    }
    selector(parent, context).is_some_and(|name| COMPARISON_OPERATORS.contains(&name))
}

/// `used_if_condition_in_body?`: the nearest enclosing conditional tests this very read.
fn used_if_condition_in_body(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = context.parent(node);
    let condition = loop {
        let Some(ancestor) = current else {
            return false;
        };
        if matches!(
            ancestor.kind_str(),
            "if" | "unless" | "elsif" | "conditional" | "if_modifier" | "unless_modifier"
        ) {
            match ancestor.field("condition") {
                Some(condition) => break condition,
                None => return false,
            }
        }
        current = context.parent(ancestor);
    };
    // `condition.child_nodes == node.child_nodes`: the condition reads the same two things.
    if is_send(condition, context) && same_children(condition, node, context) {
        return true;
    }
    used_in_condition(node, condition, context)
}

/// `send_type?`: a call, which `&&` and `||` are not.
///
/// The grammar writes a logical operator with the same node it writes an operator call with, but
/// upstream's parser has `and` and `or` nodes of their own for them. Reading `ENV['X'] && y` as a
/// call asks it for a method name, decides `&&` is neither a comparison nor a predicate, and stops
/// before noticing that the read is one of the two things the condition tests.
fn is_send(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !SEND_KINDS.contains(&node.kind_str()) {
        return false;
    }
    node.kind_str() != "binary"
        || node
            .field("operator")
            .is_none_or(|operator| !LOGICAL_OPERATORS.contains(&context.source.node_text(operator)))
}

/// `used_in_condition?`.
fn used_in_condition(node: Node<'_>, condition: Node<'_>, context: &RuleContext<'_>) -> bool {
    if is_send(condition, context) {
        let name = selector(condition, context);
        // `assignment_method?`: a setter, which is not one of the comparisons.
        let assignment = name.is_some_and(|name| {
            name.ends_with('=') && !COMPARISON_OPERATORS.contains(&name) && name != "!="
        });
        if assignment && partial_matched(node, condition, context) {
            return true;
        }
        let comparison = name.is_some_and(|name| COMPARISON_OPERATORS.contains(&name));
        let predicate = name.is_some_and(|name| name.ends_with('?'));
        if !comparison && !predicate {
            return false;
        }
    }
    child_nodes(condition)
        .into_iter()
        .any(|child| super::nodes::same_tree(context, child, node))
}

/// `partial_matched?`: every child of the read appears among the condition's own.
fn partial_matched(node: Node<'_>, condition: Node<'_>, context: &RuleContext<'_>) -> bool {
    let theirs = child_nodes(condition);
    child_nodes(node).into_iter().all(|mine| {
        theirs
            .iter()
            .any(|other| super::nodes::same_tree(context, mine, *other))
    })
}

/// `message_chained_with_dot?`: the read is the receiver of a call written with a dot.
fn message_chained_with_dot(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = upstream_parent(node, context) else {
        return false;
    };
    if parent.kind_str() != "call" {
        return false;
    }
    if parent
        .field("receiver")
        .is_none_or(|receiver| receiver.id() != node.id())
    {
        return false;
    }
    parent
        .field("operator")
        .is_some_and(|dot| matches!(context.source.node_text(dot), "." | "&."))
}

/// `assigned?`: the read is the target of an assignment, which is what `ENV['X'] ||= 'y'` is.
fn assigned(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = upstream_parent(node, context) else {
        return false;
    };
    match parent.kind_str() {
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_some_and(|left| left.id() == node.id()),
        // Every target of a multiple assignment is written into a list of its own. Writing to a
        // subscript is `:[]=` upstream, which `RESTRICT_ON_SEND` never asks the cop about.
        "left_assignment_list" | "rest_assignment" | "destructured_left_assignment" => true,
        _ => false,
    }
}

/// `or_lhs?`: the read is the left of an `||`, or sits anywhere in a chain of them.
fn or_lhs(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = upstream_parent(node, context) else {
        return false;
    };
    if !is_or(parent, context) {
        return false;
    }
    parent
        .field("left")
        .is_some_and(|left| left.id() == node.id())
        || context
            .parent(parent)
            .is_some_and(|grandparent| is_or(grandparent, context))
}

fn is_or(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "||" | "or"))
}

/// The selector upstream's `send` carries, which is the operator of a binary or unary here.
fn selector<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "call" => node
            .field("method")
            .map(|name| context.source.node_text(name)),
        "binary" => node
            .field("operator")
            .map(|operator| context.source.node_text(operator)),
        "element_reference" => Some("[]"),
        _ => None,
    }
}

/// `node.child_nodes`: the children upstream's parser keeps as nodes, with the method name -- a
/// bare symbol there -- left out.
fn child_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    match node.kind_str() {
        "call" => {
            let mut found: Vec<Node<'tree>> = node.field("receiver").into_iter().collect();
            found.extend(arguments(node).iter().map(|argument| argument.first()));
            found
        }
        "binary" => [node.field("left"), node.field("right")]
            .into_iter()
            .flatten()
            .collect(),
        "unary" => node.field("operand").into_iter().collect(),
        _ => children(node),
    }
}

fn same_children(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (theirs, mine) = (child_nodes(left), child_nodes(right));
    theirs.len() == mine.len()
        && theirs
            .iter()
            .zip(&mine)
            .all(|(left, right)| super::nodes::same_tree(context, *left, *right))
}

/// `node.parent` as upstream builds it: the argument list the grammar adds has no counterpart.
fn upstream_parent<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let parent = context.parent(node)?;
    match parent.kind_str() {
        "argument_list" => context.parent(parent),
        _ => Some(parent),
    }
}

fn children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    super::nodes::children(node)
}
