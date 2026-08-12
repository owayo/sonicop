use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::percent_literal::{percent_type, value_text, values};

const MSG: &str = "Within `%w`/`%W`, quotes and ',' are unnecessary and may be unwanted in the resulting strings.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("string_array") {
        if !matches!(percent_type(node, context), Some("%w" | "%W")) {
            continue;
        }
        let words = values(node);
        if !words
            .iter()
            .any(|word| value_text(*word, context).is_some_and(quoted_or_comma))
        {
            continue;
        }
        let mut edits: Vec<Edit> = Vec::new();
        for word in &words {
            let text = context.source.node_text(*word);
            // `remove_trailing(range, match.length)` where the pattern is `['"]?,?$`, which
            // always matches -- possibly nothing at all.
            let trailing = trailing_length(text);
            if trailing > 0 {
                edits.push(remove(word.end_byte() - trailing..word.end_byte()));
            }
            if text.starts_with(['\'', '"']) {
                edits.push(remove(word.start_byte()..word.start_byte() + 1));
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

/// `QUOTES_AND_COMMAS`: a word ending in a comma, or one wrapped in quotes of either kind. A word
/// holding no alphanumeric character at all is skipped, since a lone `'` is more likely deliberate.
fn quoted_or_comma(text: &str) -> bool {
    if !text.chars().any(char::is_alphanumeric) {
        return false;
    }
    text.ends_with(',')
        || (text.starts_with('\'') && text.ends_with('\'') && text.len() > 1)
        || (text.starts_with('"') && text.ends_with('"') && text.len() > 1)
}

/// The length of the `['"]?,?$` match at the end of the word.
fn trailing_length(text: &str) -> usize {
    let mut length = 0;
    let mut rest = text;
    if let Some(head) = rest.strip_suffix(',') {
        length += 1;
        rest = head;
    }
    if rest.ends_with(['\'', '"']) {
        length += 1;
    }
    length
}
