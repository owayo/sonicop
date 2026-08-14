use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "`#to_json` requires an optional argument to be parsable via JSON.generate(obj).";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name) = node.field("name") else {
            continue;
        };
        if context.source.node_text(name) != "to_json" {
            continue;
        }
        let parameters = node.field("parameters");
        if parameters.is_some_and(|parameters| parameters.named_child_count() > 0) {
            continue;
        }
        // `insert_after` hangs the text off the range the cop passed rather than off the range it
        // reported, which is the whole definition here.
        let (anchor, insertion) = match parameters.and_then(|parameters| parameters.child(0)) {
            // Explicit empty parentheses already stand where the argument goes.
            Some(open) if open.kind_str() == "(" => (open.byte_range(), "*_args"),
            _ => (name.byte_range(), "(*_args)"),
        };
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrections_anchored_at(anchor.clone())
                .corrected_by(Edit {
                    start: anchor.end,
                    end: anchor.end,
                    replacement: insertion.to_owned(),
                    safe: true,
                }),
        );
    }
}
