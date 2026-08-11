use tree_sitter::Node;

use super::support::{HeredocEnds, LengthTarget, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(25);
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of_any(&["block", "do_block"]) {
        if block_method_allowed(node, context, &allowed) || class_constructor(node, context) {
            continue;
        }
        report_length(
            context,
            offenses,
            node,
            max,
            "Block",
            LengthTarget::Block,
            &heredocs,
        );
    }
}

/// The method the block is passed to. A lambda literal has no call of its own; RuboCop still sees
/// a block there, and names the method `lambda`.
fn block_method_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let parent = node.parent()?;
    if parent.kind() == "lambda" {
        return Some("lambda");
    }
    let call = parent.kind().eq("call").then_some(parent)?;
    Some(
        context
            .source
            .node_text(call.child_by_field_name("method")?),
    )
}

fn block_receiver<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let call = node.parent().filter(|parent| parent.kind() == "call")?;
    Some(
        context
            .source
            .node_text(call.child_by_field_name("receiver")?),
    )
}

/// `AllowedMethods` entries are either a bare method name or `Receiver.method`; the second form
/// only exempts the block when the receiver written at the call site matches too.
fn block_method_allowed(node: Node<'_>, context: &RuleContext<'_>, allowed: &[String]) -> bool {
    let Some(method) = block_method_name(node, context) else {
        return false;
    };
    let receiver = block_receiver(node, context);
    allowed.iter().any(|entry| match entry.split_once('.') {
        Some((entry_receiver, entry_method)) => {
            entry_method == method && Some(entry_receiver) == receiver
        }
        None => entry == method,
    })
}

/// `Class.new`/`Module.new`/`Struct.new`/`Data.define` bodies are class definitions in disguise,
/// which RuboCop measures with `Metrics/ClassLength` instead. The constant has to be the global
/// one, so a namespaced `Foo::Class` is not exempt.
fn class_constructor(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(call) = node.parent().filter(|parent| parent.kind() == "call") else {
        return false;
    };
    let (Some(receiver), Some(method)) = (
        call.child_by_field_name("receiver"),
        call.child_by_field_name("method"),
    ) else {
        return false;
    };
    if !matches!(receiver.kind(), "constant" | "scope_resolution") {
        return false;
    }
    let text = context.source.node_text(receiver);
    let name = text.strip_prefix("::").unwrap_or(text);
    if name.contains("::") {
        return false;
    }
    match context.source.node_text(method) {
        "new" => matches!(name, "Class" | "Module" | "Struct"),
        "define" => name == "Data",
        _ => false,
    }
}
