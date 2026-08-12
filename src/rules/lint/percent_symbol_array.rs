use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::percent_literal::{percent_type, value_text, values};

const MSG: &str =
    "Within `%i`/`%I`, ':' and ',' are unnecessary and may be unwanted in the resulting symbols.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("symbol_array") {
        if !matches!(percent_type(node, context), Some("%i" | "%I")) {
            continue;
        }
        let symbols = values(node);
        if !symbols
            .iter()
            .any(|symbol| value_text(*symbol, context).is_some_and(colon_or_comma))
        {
            continue;
        }
        let mut edits: Vec<Edit> = Vec::new();
        for symbol in &symbols {
            let text = context.source.node_text(*symbol);
            if text.ends_with(',') {
                edits.push(remove(symbol.end_byte() - 1..symbol.end_byte()));
            }
            if text.starts_with(':') {
                edits.push(remove(symbol.start_byte()..symbol.start_byte() + 1));
            }
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

fn remove(range: std::ops::Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// A symbol whose name carries the punctuation that separates the others. One holding no
/// alphanumeric character is skipped: `%i[- ,]` names symbols that are punctuation on purpose.
fn colon_or_comma(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric) && (text.starts_with(':') || text.ends_with(','))
}
