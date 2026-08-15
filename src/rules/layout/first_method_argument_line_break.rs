use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::element_line_breaks::{expanded_arguments, indices, is_assignment_target, method_line_break};

const MSG: &str = "Add a line break before the first argument of a multi-line method argument list.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    // `on_send`, `on_csend` and `on_super` between them see every call; an index read is a `:[]`
    // send there as much as a named call is.
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let children = match node.kind_str() {
            "element_reference" => match is_assignment_target(node) {
                true => continue,
                false => indices(node),
            },
            _ => {
                let name = node
                    .field("method")
                    .map(|method| context.source.node_text(method))
                    .unwrap_or_default();
                if allowed.iter().any(|method| method == name) {
                    continue;
                }
                expanded_arguments(node)
            }
        };
        if children.is_empty() {
            continue;
        }
        offenses.extend(method_line_break(
            context, node, &children, ignore_last, MSG,
        ));
    }
}
