use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range, symbol_name};

use super::literals::{is_constant, literal_type};
use crate::rules::node_ext::NodeExt;

const SEND_METHODS: &[&str] = &["send", "public_send", "__send__"];
const MIXIN_METHODS: &[&str] = &["include", "prepend", "extend"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if !is_plain_send(call, context) {
            continue;
        }
        let Some(selector) = call.field("method") else {
            continue;
        };
        if !SEND_METHODS.contains(&context.source.node_text(selector)) {
            continue;
        }
        // `{nil? self (const _ _)}`: the mixin has to be reached through the module itself.
        if call
            .field("receiver")
            .is_some_and(|receiver| receiver.kind_str() != "self" && !is_constant(receiver, context))
        {
            continue;
        }
        let call_arguments = arguments(call);
        let [name, modules @ ..] = call_arguments.as_slice() else {
            continue;
        };
        if modules.is_empty() {
            continue;
        }
        let Some(method) = mixin_method(name.first(), context) else {
            continue;
        };
        if !modules
            .iter()
            .all(|module| module.parts().len() == 1 && is_constant(module.first(), context))
        {
            continue;
        }
        let names = modules
            .iter()
            .map(|module| context.source.node_text(module.first()))
            .collect::<Vec<&str>>()
            .join(", ");
        let range = selector.start_byte()..send_range(call, context).end;
        let replacement = format!("{method} {names}");
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{replacement}` instead of `{}`.",
                        context.source.slice(range.clone())
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `({sym str} $#mixin_method?)`: the name of one of the three mixin methods, written either way.
fn mixin_method<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let name = match literal_type(node, context)? {
        "sym" => symbol_name(node, context)?,
        "str" => crate::rules::send_node::string_text(node, context),
        _ => return None,
    };
    MIXIN_METHODS.contains(&name).then_some(name)
}
