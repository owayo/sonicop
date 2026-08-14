use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `BAD_ARGUMENTS`: the three ways of writing "split on nothing", compared by source text just as
/// upstream does.
const BAD_ARGUMENTS: [&str; 3] = ["//", "''", "\"\""];

/// `RESTRICT_ON_SEND = %i[split]` is the whole entry condition: the receiver is never looked at, so
/// a bare `split('')` is reported too.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "split" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let [only] = arguments.as_slice() else {
            continue;
        };
        if !BAD_ARGUMENTS.contains(&context.source.node_text(*only)) {
            continue;
        }
        // `range_between(node.loc.selector.begin_pos, node.source_range.end_pos)`: the selector
        // through the end of the call, so the receiver stays and a block written after it is left
        // outside.
        let range = selector.start_byte()..send_node::send_range(node, context).end;
        let current = &context.source.text()[range.clone()];
        offenses.push(
            context
                .offense(
                    format!("Use `chars` instead of `{current}`."),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: "chars".to_owned(),
                    safe: true,
                }),
        );
    }
}
