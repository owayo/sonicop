use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str =
    "Place the first line of a multi-line method definition's body on its own line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // `node.endless?`: a definition without an `end` has its body on the signature's line by
        // construction.
        if !node
            .child(node.child_count().saturating_sub(1) as u32)
            .is_some_and(|last| last.kind() == "end")
        {
            continue;
        }
        super::trailing_body::check(context, offenses, node, MSG);
    }
}
