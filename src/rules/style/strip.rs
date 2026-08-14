use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(outer) = selector(context, node) else {
            continue;
        };
        let inner_name = match outer.1 {
            "lstrip" => "rstrip",
            "rstrip" => "lstrip",
            _ => continue,
        };
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let Some(inner) = selector(context, receiver) else {
            continue;
        };
        if inner.1 != inner_name {
            continue;
        }
        let range = inner.0.start_byte()..node.end_byte();
        let message = format!(
            "Use `strip` instead of `{}`.",
            &context.source.text()[range.clone()]
        );
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: "strip".to_owned(),
            safe: true,
        }));
    }
}

/// `(call _ :name)`: the selector of a call taking neither arguments nor a block.
fn selector<'a, 'tree>(
    context: &'a RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'a str)> {
    if node.kind_str() != "call"
        || node.field("arguments").is_some()
        || node.field("block").is_some()
    {
        return None;
    }
    let method = node.field("method")?;
    Some((method, context.source.node_text(method)))
}
