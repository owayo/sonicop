use tree_sitter::Node;

use super::support::{HeredocEnds, LengthTarget, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(10);
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        if node.field("name").is_some_and(|name| {
            allowed
                .iter()
                .any(|entry| entry == context.source.node_text(name))
        }) {
            continue;
        }
        report_length(
            context,
            offenses,
            node,
            max,
            "Method",
            LengthTarget::Body,
            &heredocs,
        );
    }
    // A `define_method` block defines a method just as much as `def` does, so RuboCop measures it
    // here as well -- under the `Method` label, and reported against the `define_method` call.
    for node in context.nodes_of_any(&["block", "do_block"]) {
        if !defines_method(context, node, &allowed) {
            continue;
        }
        report_length(
            context,
            offenses,
            node,
            max,
            "Method",
            LengthTarget::Block,
            &heredocs,
        );
    }
}

fn defines_method(context: &RuleContext<'_>, node: Node<'_>, allowed: &[String]) -> bool {
    let Some(call) = node.parent().filter(|parent| parent.kind_str() == "call") else {
        return false;
    };
    if call
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "define_method")
    {
        return false;
    }
    // The defined name is the call's first argument. Only a literal name can be matched against
    // `AllowedMethods`; anything computed is measured like every other method.
    let Some(name) = call
        .field("arguments")
        .and_then(|arguments| arguments.named_child(0))
        .filter(|argument| matches!(argument.kind_str(), "simple_symbol" | "string"))
    else {
        return true;
    };
    let literal = context.source.node_text(name).trim_start_matches(':');
    let literal = literal.trim_matches(['"', '\'']);
    !allowed.iter().any(|entry| entry == literal)
}
