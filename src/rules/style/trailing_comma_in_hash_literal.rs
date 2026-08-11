use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::{nodes, trailing_comma};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("hash") {
        let items = nodes::children(node);
        let Some(last) = items.last() else {
            continue;
        };
        // A braceless hash is the tail of an argument list, and is checked as one there.
        let Some(closing) = trailing_comma::closing_bracket(node, "}") else {
            continue;
        };
        trailing_comma::check(
            context,
            &items,
            "item of %<article>s hash",
            last.end_byte(),
            closing.start_byte(),
            offenses,
        );
    }
}
