use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::statements::statements;

const MSG: &str = "Empty `ensure` block detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("ensure") {
        if !statements(node).is_empty() {
            continue;
        }
        let Some(keyword) = node.child(0) else {
            continue;
        };
        // `corrector.remove(node.loc.keyword)`: the word alone, which leaves the line it was on.
        offenses.push(
            context
                .offense(MSG, keyword.byte_range())
                .corrected_by(Edit {
                    start: keyword.start_byte(),
                    end: keyword.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
