use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::percent_literal::{percent_type, value_text, values};

const MSG: &str = "Within percent literals, nested percent literals do not function and may be unwanted in the result.";

/// `PreferredDelimiters::PERCENT_LITERAL_TYPES`, which is what an element has to start with -- and
/// then something other than a word character, as `/\A%w\W/` demands.
const PERCENT_LITERAL_TYPES: &[&str] = &["%", "%i", "%I", "%q", "%Q", "%r", "%s", "%w", "%W", "%x"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["array", "string_array", "symbol_array"]) {
        if percent_type(node, context).is_none() {
            continue;
        }
        let nested = values(node)
            .into_iter()
            .any(|value| value_text(value, context).is_some_and(starts_a_percent_literal));
        if nested {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}

/// `/\A#{type}\W/`: the element opens with a percent literal prefix followed by a delimiter.
fn starts_a_percent_literal(text: &str) -> bool {
    PERCENT_LITERAL_TYPES.iter().any(|prefix| {
        text.strip_prefix(prefix).is_some_and(|rest| {
            rest.chars()
                .next()
                .is_some_and(|first| !first.is_alphanumeric() && first != '_')
        })
    })
}
