//! `UncommunicativeName`, shared by the two cops that judge how descriptive a parameter name is.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{Parameter, ParameterKind, parameter_full_name, parameters};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::source::SourceFile;

pub(super) fn check(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    list: Node<'_>,
    name_type: &str,
) {
    let allowed: Vec<String> = context.setting("AllowedNames").unwrap_or_default();
    let forbidden: Vec<String> = context.setting("ForbiddenNames").unwrap_or_default();
    let allow_numbers: bool = context.setting("AllowNamesEndingInNumbers").unwrap_or(true);
    let min_length: usize = context.setting("MinNameLength").unwrap_or(0);
    // `add_offense` refuses a range it has already reported, and one argument can break more than
    // one rule at once -- `aB` is both upper case and too short -- so only the first message for a
    // range survives.
    let mut reported: HashSet<Range<usize>> = HashSet::new();

    for parameter in parameters(list) {
        let Some(full_name) = parameter_full_name(&parameter, context.source) else {
            continue;
        };
        // A leading underscore marks an argument as unused and is not part of the name being
        // judged; a bare `_` is left alone entirely.
        if full_name == "_" {
            continue;
        }
        let name = full_name.trim_start_matches('_');
        if allowed.iter().any(|entry| entry == name) {
            continue;
        }
        let range = argument_range(&parameter, &full_name, context.source);
        let message = if forbidden.iter().any(|entry| entry == name) {
            format!("Do not use {name} as a name for a {name_type}.")
        } else if name.chars().any(char::is_uppercase) {
            format!("Only use lowercase characters for {name_type}.")
        } else if name.chars().count() < min_length {
            format!(
                "{} must be at least {min_length} characters long.",
                capitalize(name_type)
            )
        } else if !allow_numbers
            && name
                .chars()
                .next_back()
                .is_some_and(|last| last.is_ascii_digit())
        {
            format!("Do not end {name_type} with a number.")
        } else {
            continue;
        };
        if reported.insert(range.clone()) {
            offenses.push(context.offense(message, range));
        }
    }
}

/// `arg_range`: the name's length counted in characters from the *argument's* start, so the range
/// runs over the sigil of a `*` or `**` argument and stops one character into the name of a `&`
/// one. A destructured argument measures the length of an S-expression, so the range can reach
/// past the argument and even past the line it was written on.
fn argument_range(parameter: &Parameter<'_>, full_name: &str, source: &SourceFile) -> Range<usize> {
    let length = full_name.chars().count()
        + match parameter.kind {
            ParameterKind::Restarg => 1,
            ParameterKind::Kwrestarg => 2,
            _ => 0,
        };
    let start = parameter.node.start_byte();
    let end = source.text()[start..]
        .char_indices()
        .nth(length)
        .map_or(source.len(), |(offset, _)| start + offset);
    start..end
}

/// `String#capitalize`: the first character upper-cased and the rest left alone.
fn capitalize(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}
