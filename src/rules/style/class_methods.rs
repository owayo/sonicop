use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["class", "module"]) {
        let Some(name) = node.field("name") else {
            continue;
        };
        let Some(key) = constant_key(context, name) else {
            continue;
        };
        let Some(body) = node.field("body") else {
            continue;
        };
        // Only the statements the body holds directly are looked at: a definition nested inside a
        // `begin` or a conditional is not a child of the class body upstream either.
        for definition in super::nodes::children(body) {
            if definition.kind_str() != "singleton_method" {
                continue;
            }
            let Some(receiver) = definition.field("object") else {
                continue;
            };
            if constant_key(context, receiver).as_deref() != Some(key.as_str()) {
                continue;
            }
            let Some(method) = definition.field("name") else {
                continue;
            };
            // `node.receiver.loc.name` is the last segment of a qualified constant, not the whole
            // of it.
            let reported = match receiver.field("name") {
                Some(last) => last,
                None => receiver,
            };
            let message = format!(
                "Use `self.{method}` instead of `{class_name}.{method}`.",
                method = context.source.node_text(method),
                class_name = context.source.node_text(receiver),
            );
            offenses.push(
                context
                    .offense(message, reported.byte_range())
                    .corrected_by(Edit {
                        start: receiver.start_byte(),
                        end: receiver.end_byte(),
                        replacement: "self".to_owned(),
                        safe: true,
                    }),
            );
        }
    }
}

/// A constant path in a form two of them can be compared in, which is what upstream's `==` on the
/// two nodes amounts to. `A::B` and `::A::B` name the same class but are not the same node.
fn constant_key(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node).to_owned()),
        "scope_resolution" => {
            let name = context.source.node_text(node.field("name")?);
            match node.field("scope") {
                Some(scope) => Some(format!("{}::{name}", constant_key(context, scope)?)),
                None => Some(format!("::{name}")),
            }
        }
        _ => None,
    }
}
