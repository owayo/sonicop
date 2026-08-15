use std::collections::HashSet;

use crate::diagnostic::{Edit, Offense};
use crate::directives::directive_cop_names;
use crate::rules::RuleContext;

const MSG: &str = "RuboCop disable/enable directives are not permitted.";

/// A `# rubocop:disable` written to make an offense go away rather than fixing it.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = names(context, "AllowedCops");
    let disallowed_config = names(context, "DisallowedCops");
    for range in context.comment_ranges() {
        let text = context.source.slice(range.clone());
        let listed = directive_cop_names(text);
        let disallowed = disallowed_of(&listed, &allowed, &disallowed_config);
        if disallowed.is_empty() {
            continue;
        }
        let message = if allowed.is_empty() && disallowed_config.is_empty() {
            MSG.to_owned()
        } else {
            format!(
                "RuboCop disable/enable directives for `{}` are not permitted.",
                disallowed.join("`, `")
            )
        };
        // A directive naming something the configuration still allows keeps its comment, minus the
        // name that is not allowed; one naming nothing else goes away entirely.
        let replacement = match listed.len() == disallowed.len() {
            true => String::new(),
            false => without(text, &disallowed),
        };
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement,
            safe: true,
        }));
    }
}

/// `compute_disallowed_cops`.
fn disallowed_of<'a>(
    listed: &[&'a str],
    allowed: &HashSet<String>,
    disallowed_config: &HashSet<String>,
) -> Vec<&'a str> {
    if disallowed_config.is_empty() {
        return listed
            .iter()
            .filter(|cop| !allowed.contains(**cop))
            .copied()
            .collect();
    }
    // `DisallowedCops` names what may not be switched off, and `all` switches off everything named
    // there along with the rest.
    if listed.contains(&"all") {
        return listed.to_vec();
    }
    let mut seen = HashSet::new();
    listed
        .iter()
        .filter(|cop| seen.insert(**cop) && disallowed_config.contains(**cop))
        .copied()
        .collect()
}

/// `comment.text.sub(/#{Regexp.union(disallowed_cops)},?\s*/, '').sub(/,\s*$/, '')`.
///
/// Both substitutions replace one match, so a comment naming two cops the configuration turns down
/// keeps the second of them -- and the pass that follows reports what is left over again.
fn without(text: &str, disallowed: &[&str]) -> String {
    let Some((start, end)) = first_match(text, disallowed) else {
        return trimmed_trailing_comma(text);
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    trimmed_trailing_comma(&out)
}

/// The leftmost place any of the names is written, with the comma and blanks after it.
///
/// An alternation prefers the branch written first where several of them match at one place, which
/// is what `Regexp.union` builds and what the crate matches with.
fn first_match(text: &str, disallowed: &[&str]) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    for start in 0..=text.len() {
        if !text.is_char_boundary(start) {
            continue;
        }
        let Some(name) = disallowed
            .iter()
            .find(|name| !name.is_empty() && text[start..].starts_with(**name))
        else {
            continue;
        };
        let mut end = start + name.len();
        if bytes.get(end) == Some(&b',') {
            end += 1;
        }
        while bytes
            .get(end)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
        {
            end += 1;
        }
        return Some((start, end));
    }
    None
}

/// `sub(/,\s*$/, '')`.
fn trimmed_trailing_comma(text: &str) -> String {
    let without_blanks = text.trim_end_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c']);
    match without_blanks.strip_suffix(',') {
        Some(kept) => kept.to_owned(),
        None => text.to_owned(),
    }
}

/// `Array(cop_config[key]).to_set`.
fn names(context: &RuleContext<'_>, key: &str) -> HashSet<String> {
    context
        .setting::<Vec<String>>(key)
        .unwrap_or_default()
        .into_iter()
        .collect()
}
