use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        // `RESTRICT_ON_SEND` names the method and nothing else, so `Foo.lambda(&block)` is
        // reported and rewritten the same way a bare `lambda` is.
        if context.source.node_text(method) != "lambda" {
            continue;
        }
        // `node.parent&.block_type?`: a literal block is what the cop is asking for.
        if node.field("block").is_some() {
            continue;
        }
        let arguments = arguments(node);
        let Some(first) = arguments.first().map(|argument| argument.first()) else {
            continue;
        };
        // `(send nil? :lambda (block_pass (sym _)))`: `lambda(&:foo)` builds a lambda from a
        // symbol, and dropping the `lambda` would leave a proc that behaves differently.
        if node.field("receiver").is_none()
            && arguments.len() == 1
            && first.kind_str() == "block_argument"
            && first
                .named_child(0)
                .is_some_and(|inner| inner.kind_str() == "simple_symbol")
        {
            continue;
        }
        let range = node.byte_range();
        let replacement = context
            .source
            .node_text(first)
            .strip_prefix('&')
            .unwrap_or_else(|| context.source.node_text(first))
            .to_owned();
        offenses.push(
            context
                .offense(
                    "lambda without a literal block is deprecated; use the proc without lambda \
                     instead.",
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
