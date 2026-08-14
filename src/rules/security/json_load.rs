use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{
    any_descendant, arguments, is_plain_send, pair_key_symbol, top_level_constant,
};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(method);
        if !matches!(name, "load" | "restore") || !is_plain_send(node, context) {
            continue;
        }
        if !node
            .field("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "JSON", context))
        {
            continue;
        }
        // `(... ... !`(pair (sym :create_additions) _))`: at least one argument, whose last one
        // holds no `create_additions:` anywhere in it. Naming the option either way is a decision
        // to load additions or not, and upstream leaves both alone.
        let arguments = arguments(node);
        let Some(last) = arguments.last() else {
            continue;
        };
        if last
            .parts()
            .iter()
            .any(|part| any_descendant(*part, &mut |node| creates_additions(node, context)))
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Prefer `JSON.parse` over `JSON.{name}`."),
                    method.byte_range(),
                )
                .corrected_by(Edit {
                    start: method.start_byte(),
                    end: method.end_byte(),
                    replacement: "parse".to_owned(),
                    safe: true,
                }),
        );
    }
}

fn creates_additions(node: tree_sitter::Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "pair" && pair_key_symbol(node, context) == Some("create_additions")
}
