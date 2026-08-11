use tree_sitter::Node;

use super::support::{LengthTarget, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(25);
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    for node in context.nodes_of_any(&["block", "do_block"]) {
        if block_method_allowed(node, context, &allowed) {
            continue;
        }
        report_length(context, offenses, node, max, "Block", LengthTarget::Block);
    }
}

/// `Class.new`/`Struct.new` bodies are class definitions in disguise, which RuboCop measures with
/// `Metrics/ClassLength` instead.
fn block_method_allowed(node: Node<'_>, context: &RuleContext<'_>, allowed: &[String]) -> bool {
    let Some(call) = node.parent().filter(|parent| parent.kind() == "call") else {
        return false;
    };
    let Some(method) = call.child_by_field_name("method") else {
        return false;
    };
    if context.source.node_text(method) == "new"
        && call
            .child_by_field_name("receiver")
            .is_some_and(|receiver| {
                matches!(context.source.node_text(receiver), "Class" | "Struct")
            })
    {
        return true;
    }
    allowed
        .iter()
        .any(|name| name == context.source.node_text(method))
}
