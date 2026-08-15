use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::statements::statements;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("rescue") {
        // `(resbody nil? $(casgn _ _) nil?)`: no exception list and no body of its own. A clause
        // that catches something or does something is a `rescue` the author wrote on purpose.
        let body_is_empty = node
            .field("body")
            .is_none_or(|body| statements(body).is_empty());
        if node.field("exceptions").is_some() || !body_is_empty {
            continue;
        }
        let (Some(keyword), Some(variable)) = (node.child(0), node.field("variable")) else {
            continue;
        };
        let (Some(assoc), Some(target)) = (variable.child(0), variable.named_child(0)) else {
            continue;
        };
        if !matches!(target.kind_str(), "constant" | "scope_resolution") {
            continue;
        }
        let message = format!(
            "`{}` is overwritten by `rescue =>`.",
            context.source.node_text(target)
        );
        offenses.push(
            context
                .offense(message, assoc.byte_range())
                .corrected_by(Edit {
                    start: keyword.end_byte(),
                    end: assoc.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
