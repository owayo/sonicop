use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&[
        "identifier",
        "constant",
        "instance_variable",
        "class_variable",
        "global_variable",
    ]) {
        if context.source.node_text(node).is_ascii() {
            continue;
        }
        offenses.push(context.offense("Use only ASCII symbols in identifiers.", node.byte_range()));
    }
}
