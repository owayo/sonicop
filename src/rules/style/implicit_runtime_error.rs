use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `(send nil? ${:raise :fail} {str dstr})`: a raise handed nothing but a message, so the class it
/// raises is left implicit.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `(send nil? ...)`: the bare keyword form only.
        if node.field("receiver").is_some() {
            continue;
        }
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        if method != "raise" && method != "fail" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let [only] = arguments.as_slice() else {
            continue;
        };
        // `{str dstr}`: a string literal, interpolating or not. Adjacent literals are a `dstr` too,
        // which tree-sitter keeps as a `chained_string`.
        if !matches!(only.kind_str(), "string" | "chained_string") {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "Use `{method}` with an explicit exception class and message, \
                 rather than just a message."
            ),
            send_node::send_range(node, context),
        ));
    }
}
