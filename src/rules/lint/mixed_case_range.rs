use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal::string_value;
use crate::rules::send_node::is_string;

use super::regexp_source;

const MSG: &str = "Ranges from upper to lower case ASCII letters may include unintended \
                   characters. Instead of `A-z` (which also includes several symbols) specify \
                   each range individually: `A-Za-z` and individually specify any symbols.";

/// `RANGES`: the two the cop knows how to take a mixed range apart into.
const RANGES: [(char, char); 2] = [('a', 'z'), ('A', 'Z')];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("range") {
        // Both ends written, and both of them strings: `('A'..)` and `(1..9)` say nothing about
        // letter case.
        let (Some(lower), Some(upper)) = (node.field("begin"), node.field("end")) else {
            continue;
        };
        if !is_string(lower, context) || !is_string(upper, context) {
            continue;
        }
        if unsafe_range(&string_value(lower, context), &string_value(upper, context)) {
            // A `Range` cannot be split in two the way a character class can, so this half of the
            // cop only reports.
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
    for node in context.nodes_of("regex") {
        let Some(pattern) = regexp_source::parse(node, context) else {
            continue;
        };
        for index in pattern.tree.expressions() {
            let expression = &pattern.tree.nodes[index];
            if expression.kind != "set" || expression.token != "character" {
                continue;
            }
            for &child in &expression.children {
                // `range_pairs`: the first two members of every set inside this one, which for the
                // `a-z` the cop is after are its bounds.
                let inner = &pattern.tree.nodes[child];
                if inner.kind != "set" {
                    continue;
                }
                let (Some(&lower), Some(&upper)) = (inner.children.first(), inner.children.get(1))
                else {
                    continue;
                };
                let (lower, upper) = (&pattern.tree.nodes[lower], &pattern.tree.nodes[upper]);
                if lower.kind != "literal" || upper.kind != "literal" {
                    continue;
                }
                if !unsafe_range(&lower.text, &upper.text) {
                    continue;
                }
                let range = pattern.range(lower.ts..upper.te);
                let Some(replacement) = regexp_range(context.source.slice(range.clone())) else {
                    continue;
                };
                offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    // `SafeAutoCorrect: false`: `A-z` matched the symbols between `Z` and `a`,
                    // and `A-Za-z` no longer does.
                    replacement,
                    safe: false,
                }));
            }
        }
    }
}

/// `unsafe_range?`: two single characters that fall in different letter cases.
fn unsafe_range(lower: &str, upper: &str) -> bool {
    if lower.chars().count() != 1 || upper.chars().count() != 1 {
        return false;
    }
    range_for(lower) != range_for(upper)
}

/// `range_for`: which of `a-z` and `A-Z` holds the character, if either does.
fn range_for(text: &str) -> Option<(char, char)> {
    let mut characters = text.chars();
    let (Some(character), None) = (characters.next(), characters.next()) else {
        return None;
    };
    RANGES
        .into_iter()
        .find(|&(first, last)| (first..=last).contains(&character))
}

/// `regexp_range`: `A-z` written out as the two ranges it was meant to be.
fn regexp_range(source: &str) -> Option<String> {
    let mut parts: Vec<&str> = source.split('-').collect();
    // `String#split` drops the empty strings a trailing separator leaves behind.
    while parts.last() == Some(&"") {
        parts.pop();
    }
    let open = *parts.first()?;
    let close = *parts.get(1)?;
    let (_, open_end) = range_for(open)?;
    let (close_begin, _) = range_for(close)?;
    Some(format!(
        "{}{}",
        joined(open, &open_end.to_string()),
        joined(&close_begin.to_string(), close)
    ))
}

/// `[first, last].uniq.join('-')`: a range of one character is written as that character.
fn joined(first: &str, last: &str) -> String {
    match first == last {
        true => first.to_owned(),
        false => format!("{first}-{last}"),
    }
}
