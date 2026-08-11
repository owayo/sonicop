use std::collections::HashMap;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::percent::{LITERAL_KINDS, PercentLiteral, literal_segments};

/// The delimiter pairs `matchpairs` knows. A `%w` or `%i` literal written with one of these carries
/// both halves into the check for whether its own delimiter appears in its contents.
const MATCHING_PAIRS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let preferred: HashMap<String, String> =
        context.setting("PreferredDelimiters").unwrap_or_default();
    let default = preferred.get("default");

    for node in context.nodes_of_any(LITERAL_KINDS) {
        let Some(literal) = PercentLiteral::new(node, context) else {
            continue;
        };
        let Some((opening, closing)) =
            preferred_delimiters(&literal.percent_type, &preferred, default)
        else {
            continue;
        };
        if literal.opening == opening {
            continue;
        }
        let segments = literal_segments(node, context, &literal);
        if contains_any(&segments, &[opening, closing]) {
            continue;
        }
        // `%w` and `%i` are also left alone when they already hold the delimiter they are written
        // with, which upstream reads as the literal depending on that nesting.
        if matches!(literal.percent_type.as_str(), "%w" | "%i")
            && contains_any(&segments, &used_delimiters(literal.opening))
        {
            continue;
        }

        let text = context.source.text();
        let replacement = format!(
            "{}{opening}{}{closing}{}",
            literal.percent_type,
            &text[literal.begin.end..literal.close.start],
            &text[literal.close.end..node.end_byte()],
        );
        offenses.push(
            context
                .offense(
                    format!(
                        "`{}`-literals should be delimited by `{opening}` and `{closing}`.",
                        literal.percent_type
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The configured delimiter pair for one percent-literal type.
///
/// RuboCop fills every type in from the `default` key when one is present, so a type the user did
/// not name still has a preference; without that key only the types written out have one.
fn preferred_delimiters(
    percent_type: &str,
    preferred: &HashMap<String, String>,
    default: Option<&String>,
) -> Option<(char, char)> {
    let value = preferred.get(percent_type).or(default)?;
    let mut characters = value.chars();
    Some((characters.next()?, characters.next()?))
}

/// The delimiter pair the literal is already written with: a bracketing character brings its
/// partner, anything else stands alone.
fn used_delimiters(opening: char) -> Vec<char> {
    MATCHING_PAIRS
        .iter()
        .find(|(character, _)| *character == opening)
        .map_or_else(|| vec![opening], |(open, close)| vec![*open, *close])
}

fn contains_any(segments: &[&str], delimiters: &[char]) -> bool {
    segments.iter().any(|segment| {
        segment
            .chars()
            .any(|character| delimiters.contains(&character))
    })
}
