//! Flow-of-control statements, shared by the two cops that ask what a branch ends with.
//!
//! `Lint/UnreachableCode` and `Lint/UnreachableLoop` ask the same question of the same shapes: a
//! keyword that leaves the current scope, a `Kernel` method that raises or exits, an `if` whose two
//! branches both do, a `case` whose every branch does. They differ only in which keywords count and
//! in what they do with the answer.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::send_node::{is_plain_send, top_level_constant};

use super::locals::LocalVariables;
use super::statements::Branch;
use crate::rules::node_ext::NodeExt;

/// The methods a flow command is spelled with, all of which a file may redefine.
const REDEFINABLE: [&str; 6] = ["raise", "fail", "throw", "exit", "exit!", "abort"];

/// The keywords `Lint/UnreachableCode` counts. `Lint/UnreachableLoop` takes the first two only:
/// `next` and `redo` start the next iteration rather than ending the loop, and a `retry` is no
/// keyword a loop body may hold.
const FLOW_KEYWORDS: [&str; 5] = ["return", "next", "break", "retry", "redo"];
const BREAK_KEYWORDS: [&str; 2] = ["return", "break"];

/// Whether the node is one of `Lint/UnreachableCode`'s flow commands.
pub(super) fn is_command(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    FLOW_KEYWORDS.contains(&node.kind_str()) || is_kernel_command(node, context, locals)
}

/// Whether the node is one of `Lint/UnreachableLoop`'s break commands.
pub(super) fn is_break_command(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    BREAK_KEYWORDS.contains(&node.kind_str()) || is_kernel_command(node, context, locals)
}

/// `(send {nil? (const {nil? cbase} :Kernel)} {:raise :fail :throw :exit :exit! :abort} ...)`.
///
/// A bare `raise` is a receiverless send upstream and an `identifier` here, so the name has to be
/// resolved before it can be told from a local variable that shadows it.
fn is_kernel_command(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    if node.kind_str() == "identifier" {
        return REDEFINABLE.contains(&context.source.node_text(node)) && !locals.is_lvar(node);
    }
    node.kind_str() == "call"
        && is_plain_send(node, context)
        && node
            .field("method")
            .is_some_and(|method| REDEFINABLE.contains(&context.source.node_text(method)))
        && node
            .field("receiver")
            .is_none_or(|receiver| top_level_constant(receiver, "Kernel", context))
}

/// `node.if_branch && node.else_branch && flow(if_branch) && flow(else_branch)`.
///
/// `unless` swaps the two branches upstream, and a `then` written without an `else` leaves the
/// second one `nil`; either way the test is the same on both, so which is which never shows.
/// The branch is handed over whole rather than statement by statement: upstream reaches a branch
/// of several statements as a `begin`, and what a `begin` counts as differs between the two cops
/// that ask -- `Lint/UnreachableLoop` disqualifies a `break` that a `next` precedes.
pub(super) fn check_if<'tree>(
    node: Node<'tree>,
    flow: &mut impl FnMut(&Branch<'tree>) -> bool,
) -> bool {
    let consequence = Branch::of(node.field("consequence"));
    let alternative = Branch::of(node.field("alternative"));
    consequence.exists() && alternative.exists() && flow(&consequence) && flow(&alternative)
}

/// `else_branch && flow(else_branch) && branches.all? { |b| b.body && flow(b.body) }`.
pub(super) fn check_case<'tree>(
    node: Node<'tree>,
    flow: &mut impl FnMut(&Branch<'tree>) -> bool,
) -> bool {
    // The `else` of a `case` carries no field name, unlike the one of a `case ... in`.
    let mut cursor = node.walk();
    let children: Vec<Node<'tree>> = node.named_children(&mut cursor).collect();
    let otherwise = Branch::of(
        children
            .iter()
            .copied()
            .find(|child| child.kind_str() == "else"),
    );
    if !otherwise.exists() || !flow(&otherwise) {
        return false;
    }
    let branches: Vec<Node<'tree>> = children
        .into_iter()
        .filter(|child| matches!(child.kind_str(), "when" | "in_clause"))
        .collect();
    branches.into_iter().all(|branch| {
        let body = Branch::of(branch.field("body"));
        body.exists() && flow(&body)
    })
}

/// The state `Lint/UnreachableCode` carries through one file: which of the redefinable methods the
/// file has defined so far, and how deep the walk is inside an `instance_eval`.
pub(super) struct Flow {
    redefined: Vec<String>,
}

impl Flow {
    pub(super) fn new() -> Self {
        Self {
            redefined: Vec::new(),
        }
    }

    /// `register_redefinition`: a method definition that shadows one of the flow commands stops
    /// every later call to that name from ending the flow.
    pub(super) fn register_redefinition(&mut self, node: Node<'_>, context: &RuleContext<'_>) {
        let Some(name) = node.field("name") else {
            return;
        };
        let name = context.source.node_text(name);
        if REDEFINABLE.contains(&name) {
            self.redefined.push(name.to_owned());
        }
    }

    /// `report_on_flow_command?`: a keyword and an explicit `Kernel` receiver always end the flow,
    /// while a bare name may be a method this file defined -- or, inside an `instance_eval`, one
    /// the receiver defines, which the syntax tree cannot show.
    pub(super) fn reports_command(&self, node: Node<'_>, context: &RuleContext<'_>) -> bool {
        let name = match node.kind_str() {
            "identifier" => context.source.node_text(node),
            "call" if node.field("receiver").is_none() => match node.field("method") {
                Some(method) => context.source.node_text(method),
                None => return true,
            },
            // A keyword, or a call through an explicit `Kernel`, which nothing can have shadowed.
            _ => return true,
        };
        if in_instance_eval(node, context) {
            return false;
        }
        !self.redefined.iter().any(|redefined| redefined == name)
    }
}

/// Whether the node is inside a block passed to `instance_eval`, where what a bare name calls
/// depends on a receiver the syntax tree says nothing about.
fn in_instance_eval(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "block" | "do_block")
            && ancestor.parent_of(context).is_some_and(|call| {
                call.field("method")
                    .is_some_and(|method| context.source.node_text(method) == "instance_eval")
            })
        {
            return true;
        }
        current = ancestor.parent_of(context);
    }
    false
}
