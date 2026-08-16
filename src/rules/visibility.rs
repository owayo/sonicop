//! `VisibilityHelp`: which of `public`, `private` and `protected` a definition was written under.
//!
//! Ruby says it three ways -- a bare marker standing above the definition, a marker the definition
//! itself was handed to, and a marker naming the method afterwards -- and upstream reads all three
//! off the siblings a node sits among. Both the cop that orders a class body and the one that
//! chooses between `def self.` and `class << self` rest on the same answer.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::lint::access_modifier::{bare_send_name, send_name};
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, symbol_name};

/// `VISIBILITY_SCOPES`.
const VISIBILITY_SCOPES: &[&str] = &["private", "protected", "public"];

/// The node kinds the grammar adds for statement lists.
///
/// `program` is one of them. A file's top level is a statement list like any other, and a bare
/// `private` written there puts the definitions below it out of public view -- upstream walks
/// `left_siblings` and reaches it. Leaving it out made every top-level definition look public
/// (`Style/DocumentationMethod` reported 16 spec cases the upstream cop stays quiet on).
const CONTAINERS: &[&str] = &["program", "body_statement", "block_body", "then", "else"];

/// `node_visibility`.
pub(crate) fn node_visibility(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    if node.kind_str() == "method"
        && let Some(inline) = inline_visibility(node, context)
    {
        return inline;
    }
    block_visibility(node, context).unwrap_or("public")
}

/// `node_visibility_from_visibility_inline`: `private def foo` and `private :foo`.
fn inline_visibility(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    let parent = node.parent_of(context);
    // `(send nil? VISIBILITY_SCOPES def)`
    if let Some(parent) = enclosing_call(parent, node)
        && let Some(scope) = named_scope(parent, context)
        && arguments(parent).len() == 1
    {
        return Some(scope);
    }
    let name = context.source.node_text(node.field("name")?);
    // `(send nil? VISIBILITY_SCOPES (sym %method_name))`, the last one written after the definition.
    siblings(node, context)?
        .into_iter()
        .skip_while(|sibling| sibling.id() != node.id())
        .skip(1)
        .filter(|sibling| {
            // `(send nil? VISIBILITY_SCOPES (sym %method_name))`: exactly one symbol. A modifier
            // given several names -- `private :a, :b` -- matches no pattern and marks nothing.
            match arguments(*sibling).as_slice() {
                [only] => symbol_name(only.first(), context) == Some(name),
                _ => false,
            }
        })
        .filter_map(|sibling| named_scope(sibling, context))
        .last()
}

/// `node_visibility_from_visibility_block`: the last bare modifier written above the node.
fn block_visibility(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    siblings(node, context)?
        .into_iter()
        .take_while(|sibling| sibling.id() != node.id())
        .filter_map(|sibling| bare_scope(sibling, context))
        .last()
}

/// `visibility_block?`: the `private` / `protected` / `public` a bare receiverless call names. A bare
/// one reaches the grammar as an identifier rather than as a call.
fn bare_scope(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    let name = bare_send_name(node, context)?;
    VISIBILITY_SCOPES
        .iter()
        .find(|scope| **scope == name)
        .copied()
}

/// The scope a call names whether or not it was given an argument, which is what
/// `visibility_inline_on_def?` and `visibility_inline_on_method_name?` read.
fn named_scope(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    if node.field("receiver").is_some() {
        return None;
    }
    let name = send_name(node, context)?;
    VISIBILITY_SCOPES
        .iter()
        .find(|scope| **scope == name)
        .copied()
}

/// The call a node was handed to as its only argument, which is how `private def foo` reads.
fn enclosing_call<'tree>(parent: Option<Node<'tree>>, node: Node<'_>) -> Option<Node<'tree>> {
    let parent = parent?;
    let call = match parent.kind_str() {
        "argument_list" => parent.parent()?,
        "call" => parent,
        _ => return None,
    };
    (call.kind_str() == "call" && call.end_byte() >= node.end_byte()).then_some(call)
}

/// The statements the node sits among, which is what `left_siblings` and `right_siblings` walk.
pub(crate) fn siblings<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Vec<Node<'tree>>> {
    let parent = node.parent_of(context)?;
    CONTAINERS
        .contains(&parent.kind_str())
        .then(|| statements(parent))
}

/// The statements a container holds, which upstream's `begin` node has as its children.
pub(crate) fn statements<'tree>(container: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(container)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}
