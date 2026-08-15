use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::element_line_breaks::{expanded_arguments, indices, is_assignment_target, line_breaks};

const MSG: &str = "Each argument in a multi-line method call must start on a separate line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        // `return if node.method?(:[]=)`, which is the shape an index write reaches upstream as.
        let children = match node.kind_str() {
            "element_reference" => match is_assignment_target(node) {
                true => continue,
                false => indices(node),
            },
            // `on_send` and `on_csend` only: a `super` reaches upstream as a node of its own, and
            // no handler of this cop sees it.
            _ => match node
                .field("method")
                .is_some_and(|method| method.kind_str() == "super")
            {
                true => continue,
                false => expanded_arguments(node),
            },
        };
        offenses.extend(line_breaks(context, &children, ignore_last, MSG));
    }
}
