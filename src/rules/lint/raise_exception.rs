use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `StandardError` over `Exception`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context
        .setting("AllowedImplicitNamespaces")
        .unwrap_or_else(|| vec!["Gem".to_owned()]);
    for node in context.nodes_of("call") {
        if node.field("receiver").is_some()
            || node.field("block").is_some()
            || !node
                .field("method")
                .is_some_and(|method| matches!(context.source.node_text(method), "raise" | "fail"))
        {
            continue;
        }
        let Some(exception) = raised_exception(node, context) else {
            continue;
        };
        // A constant written with no `::` may name the `Exception` of the module around it, which
        // is why `raise Exception` inside `module Gem` is left alone.
        let global = exception.kind_str() == "scope_resolution";
        if !global && inside_an_allowed_namespace(node, context, &allowed) {
            continue;
        }
        let replacement = if global {
            "::StandardError"
        } else {
            "StandardError"
        };
        offenses.push(
            context
                .offense(MSG, exception.byte_range())
                .corrected_by(Edit {
                    start: exception.start_byte(),
                    end: exception.end_byte(),
                    replacement: replacement.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// The `Exception` the call raises, written either as the class itself or as `Exception.new`.
fn raised_exception<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let arguments = arguments(node);
    let first = arguments.first()?.first();
    if is_exception(first, context) {
        return Some(first);
    }
    // `(send nil? {:raise :fail} (send $(const ...) :new ...))` takes exactly one argument.
    if arguments.len() != 1 || first.kind_str() != "call" {
        return None;
    }
    if context.source.node_text(first.field("method")?) != "new"
        || first.field("block").is_some()
    {
        return None;
    }
    let receiver = first.field("receiver")?;
    is_exception(receiver, context).then_some(receiver)
}

/// `(const {cbase nil?} :Exception)`: the top-level `Exception`, with or without the `::` that
/// spells it out.
fn is_exception(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == "Exception",
        "scope_resolution" => {
            node.field("scope").is_none()
                && node
                    .field("name")
                    .is_some_and(|name| context.source.node_text(name) == "Exception")
        }
        _ => false,
    }
}

/// `implicit_namespace?`: whether any module around the call is one whose own `Exception` the
/// unqualified name would reach.
fn inside_an_allowed_namespace(
    node: Node<'_>,
    context: &RuleContext<'_>,
    allowed: &[String],
) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind_str() == "module"
            && parent.field("name").is_some_and(|name| {
                let name = context.source.node_text(name);
                allowed.iter().any(|namespace| namespace == name)
            })
        {
            return true;
        }
        current = parent.parent();
    }
    false
}
