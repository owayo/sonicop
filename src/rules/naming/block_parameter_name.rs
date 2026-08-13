use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // Only `block` reaches `on_block`: a numbered or `it` block names nothing, and upstream has no
    // handler for either.
    for node in context.nodes_of_any(&["block", "do_block", "lambda"]) {
        let Some(list) = node.field("parameters") else {
            continue;
        };
        if list.named_child_count() == 0 {
            continue;
        }
        super::uncommunicative::check(context, offenses, list, "block parameter");
    }
}
