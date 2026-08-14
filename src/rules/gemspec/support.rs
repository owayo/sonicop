//! What the `Gemspec` cops recognise as a gem specification.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, top_level_constant};

/// The block parameter of `Gem::Specification.new do |spec|`, or `None` when the call is not one.
///
/// Upstream spells this `(block (send (const (const {cbase nil?} :Gem) :Specification) :new)
/// (args (arg $_)) ...)`: the call takes no arguments of its own and the block takes exactly one
/// plain parameter, so a specification opened any other way is not one this cop knows.
pub(super) fn specification_variable<'a>(
    call: Node<'_>,
    context: &'a RuleContext<'_>,
) -> Option<&'a str> {
    if call.kind_str() != "call" || !arguments(call).is_empty() {
        return None;
    }
    let method = call.field("method")?;
    if context.source.node_text(method) != "new" {
        return None;
    }
    let receiver = call.field("receiver")?;
    if receiver.kind_str() != "scope_resolution" {
        return None;
    }
    let name = receiver.field("name")?;
    if context.source.node_text(name) != "Specification" {
        return None;
    }
    if !top_level_constant(receiver.field("scope")?, "Gem", context) {
        return None;
    }
    let parameters = call.field("block")?.field("parameters")?;
    match named_children(parameters).as_slice() {
        [only] if only.kind_str() == "identifier" => Some(context.source.node_text(*only)),
        _ => None,
    }
}

/// The variable the file's first specification block names itself by, which is the only receiver
/// `Gemspec/DuplicatedAssignment` treats as the specification.
pub(super) fn first_specification_variable<'a>(context: &'a RuleContext<'_>) -> Option<&'a str> {
    context
        .nodes_of("call")
        .find_map(|call| specification_variable(call, context))
}

/// The `Gem::Specification.new` block `node` sits inside, identified by where that block starts.
///
/// Assignments made in two different specification blocks are not duplicates of one another, so
/// upstream makes the enclosing block part of what it groups them by.
pub(super) fn enclosing_specification(node: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    let mut child = node;
    while let Some(parent) = child.parent_of(context) {
        if matches!(child.kind_str(), "do_block" | "block")
            && specification_variable(parent, context).is_some()
        {
            return Some(parent.start_byte());
        }
        child = parent;
    }
    None
}

/// Every name the file binds as a local variable.
///
/// Upstream's parser settles this while it parses: `spec.add_dependency` reaches a cop as a call on
/// an `lvar` only because `spec` was introduced as a block parameter, and the same line reaches it
/// as a call on a receiverless `send` when it was not. The syntax tree records no such distinction,
/// so the bindings a file makes are collected once and then asked by name.
pub(super) fn local_variables<'a>(context: &'a RuleContext<'_>) -> HashSet<&'a str> {
    context
        .nodes_of("identifier")
        .filter(|node| binds_a_local(*node))
        .map(|node| context.source.node_text(node))
        .collect()
}

fn binds_a_local(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind_str() {
        "block_parameters"
        | "method_parameters"
        | "lambda_parameters"
        | "destructured_parameter"
        | "left_assignment_list"
        // `rescue => error` names the exception it caught.
        | "rescue" => true,
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter" => parent.field("name") == Some(node),
        "assignment" | "operator_assignment" => parent.field("left") == Some(node),
        "for" => parent.field("pattern") == Some(node),
        _ => false,
    }
}
