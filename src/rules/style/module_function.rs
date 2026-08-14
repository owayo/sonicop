use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "module_function".to_owned());
    for module in context.nodes_of("module") {
        let Some(body) = module.field("body") else {
            continue;
        };
        let statements = super::conditional::self_statements(body);
        let has_private = statements
            .iter()
            .any(|node| receiverless_call(*node, "private", context).is_some());
        for node in &statements {
            let extend_self = receiverless_call(*node, "extend", context)
                .is_some_and(|list| matches!(list.as_slice(), [only] if only == "self"));
            let module_function = receiverless_call(*node, "module_function", context)
                .is_some_and(|list| list.is_empty());
            let (message, replacement) = match style.as_str() {
                // `check_module_function`: a module that hides part of itself is not asking for
                // `module_function`, which would make every method a singleton one.
                "module_function" if extend_self && !has_private => (
                    "Use `module_function` instead of `extend self`.",
                    "module_function",
                ),
                "extend_self" if module_function => (
                    "Use `extend self` instead of `module_function`.",
                    "extend self",
                ),
                "forbidden" if extend_self || module_function => {
                    ("Do not use `module_function` or `extend self`.", "")
                }
                _ => continue,
            };
            let offense = context.offense(message, node.byte_range());
            offenses.push(match style == "forbidden" {
                true => offense,
                false => offense.corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: replacement.to_owned(),
                    safe: true,
                }),
            });
        }
    }
}

/// `(send nil? :name ...)`: the sources of the arguments of a receiverless call to `name`, or
/// `None` when the node is not one.
fn receiverless_call(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> Option<Vec<String>> {
    let (kind, method) = match node.kind_str() {
        // `module_function` and `private` written bare are an identifier here rather than a call.
        "identifier" => ("identifier", node),
        "call" => ("call", node.field("method")?),
        _ => return None,
    };
    if context.source.node_text(method) != name {
        return None;
    }
    if kind == "call" && node.field("receiver").is_some() {
        return None;
    }
    Some(
        arguments(node)
            .iter()
            .map(|argument| context.source.slice(argument.range()).to_owned())
            .collect(),
    )
}
