use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::{nodes, trailing_comma};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("array") {
        let items = nodes::children(node);
        let Some(last) = items.last() else {
            continue;
        };
        let Some(closing) = trailing_comma::closing_bracket(node, "]") else {
            continue;
        };
        trailing_comma::check(
            context,
            &items,
            "item of %<article>s array",
            last.end_byte(),
            closing.start_byte(),
            offenses,
        );
    }
}
