use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, named_children};

use super::locals::LocalVariables;
use super::statements::statements;

/// The receiverless calls whose arguments coerce on their own.
const PRINTERS: &[&str] = &["print", "puts", "warn"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    // `Interpolation#on_dstr` and its aliases reach every `begin` written inside a literal, in
    // whichever of the four kinds of literal can hold one.
    for interpolation in context.nodes_of("interpolation") {
        let Some(last) = statements(interpolation).last().copied() else {
            continue;
        };
        if let Some(offense) = coercion(last, "interpolation", context, &locals) {
            offenses.push(offense);
        }
    }
    for call in context.nodes_of("call") {
        if call.child_by_field_name("receiver").is_some() {
            continue;
        }
        let Some(selector) = call.child_by_field_name("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !PRINTERS.contains(&name) {
            continue;
        }
        let context_name = format!("`{name}`");
        // `each_child_node(:call)`: an argument that *is* the coercion, not one holding it.
        let Some(list) = call.child_by_field_name("arguments") else {
            continue;
        };
        for argument in named_children(list) {
            if let Some(offense) = coercion(argument, &context_name, context, &locals) {
                offenses.push(offense);
            }
        }
    }
}

/// `to_s_without_args?` plus the offense it leads to: the selector is reported, and the whole call
/// is replaced by what it was called on.
fn coercion(
    node: Node<'_>,
    place: &str,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Offense> {
    // A bare `to_s` is a receiverless send upstream and an `identifier` here, which is the whole
    // call as well as its selector.
    if node.kind() == "identifier" {
        if context.source.node_text(node) != "to_s" || locals.is_lvar(node) {
            return None;
        }
        return Some(
            context
                .offense(
                    format!("Use `self` instead of `Object#to_s` in {place}."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: "self".to_owned(),
                    safe: true,
                }),
        );
    }
    if node.kind() != "call" || !arguments(node).is_empty() {
        return None;
    }
    let selector = node.child_by_field_name("method")?;
    if context.source.node_text(selector) != "to_s" {
        return None;
    }
    // A block turns the call into a `block` node upstream, which the pattern never matches.
    if node.child_by_field_name("block").is_some() {
        return None;
    }
    let receiver = node.child_by_field_name("receiver");
    let message = match receiver {
        Some(_) => format!("Redundant use of `Object#to_s` in {place}."),
        None => format!("Use `self` instead of `Object#to_s` in {place}."),
    };
    let replacement = receiver.map_or_else(
        || "self".to_owned(),
        |receiver| context.source.node_text(receiver).to_owned(),
    );
    Some(
        context
            .offense(message, selector.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
    )
}
