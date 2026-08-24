use tree_sitter::Node;

use super::support::{HeredocEnds, LengthTarget, constructor_call, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(25);
    let allowed = crate::rules::support::allowed_methods(context);
    // `matches_allowed_pattern?(node.method_name)`: the patterns are matched against the method
    // name alone, never against the receiver.
    let patterns = crate::rules::naming::support::forbidden_patterns_named(context, "AllowedPatterns");
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of_any(&["block", "do_block"]) {
        if block_method_allowed(node, context, &allowed)
            || block_method_name(node, context)
                .is_some_and(|name| patterns.iter().any(|pattern| pattern.is_match(name)))
            || class_constructor(node, context)
        {
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
    let parent = node.parent_of(context)?;
    if parent.kind_str() == "lambda" {
        return Some("lambda");
    }
    let call = parent.kind_str().eq("call").then_some(parent)?;
    Some(
        context
            .source
            .node_text(call.field("method")?),
    )
}

/// `node.receiver&.source&.gsub(/\s+/, '')`: the receiver as written, with every run of
/// whitespace removed -- a chain broken over lines (`Foo::\n  Bar.baz`) names the same receiver
/// as the one written on a single line.
fn block_receiver(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let call = node
        .parent_of(context)
        .filter(|parent| parent.kind_str() == "call")?;
    let written = context.source.node_text(call.field("receiver")?);
    Some(written.split_whitespace().collect())
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
            entry_method == method && receiver.as_deref() == Some(entry_receiver)
        }
        None => entry == method,
    })
}

/// `Class.new`/`Module.new`/`Struct.new`/`Data.define` bodies are class definitions in disguise,
/// which RuboCop measures with `Metrics/ClassLength` instead. The constant has to be the global
/// one, so a namespaced `Foo::Class` is not exempt.
fn class_constructor(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(call) = node.parent_of(context).filter(|parent| parent.kind_str() == "call") else {
        return false;
    };
    matches!(
        constructor_call(context, call),
        Some(("Class" | "Module" | "Struct", "new") | ("Data", "define"))
    )
}
