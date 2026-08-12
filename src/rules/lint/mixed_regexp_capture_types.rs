use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::regexp::{captures, interpolates, pattern};

const MSG: &str = "Do not mix named captures and numbered captures in a Regexp literal.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("regex") {
        if interpolates(node) {
            continue;
        }
        let Some((source, extended)) = pattern(node, context) else {
            continue;
        };
        let found = captures(source, extended);
        if found.numbered > 0 && found.named > 0 {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}
