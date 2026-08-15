use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::directives::{DirectiveComment, DirectiveMode};
use crate::rules::RuleContext;

const MSG: &str = "RuboCop disable/enable directives are not permitted.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedCops").unwrap_or_default();
    let disallowed_config: Vec<String> = context.setting("DisallowedCops").unwrap_or_default();
    for comment in context.comment_ranges() {
        let line = context.source.line_column(comment.start).0;
        let Some(directive) = DirectiveComment::parse(context.source, comment.clone(), line) else {
            continue;
        };
        let named = directive_cops(&directive);
        let disallowed = compute_disallowed(&named, &allowed, &disallowed_config);
        if disallowed.is_empty() {
            continue;
        }
        offenses.push(offense(
            comment,
            &named,
            &disallowed,
            !allowed.is_empty() || !disallowed_config.is_empty(),
            context,
        ));
    }
}

/// `directive_cops`: the names the directive lists, as they were written. A `push` or a `pop` carries
/// its arguments in a group of its own, which the cop never reads.
fn directive_cops(directive: &DirectiveComment) -> Vec<String> {
    if matches!(directive.mode, DirectiveMode::Push | DirectiveMode::Pop) {
        return Vec::new();
    }
    directive
        .raw_cop_names()
        .into_iter()
        .map(|name| name.trim().to_owned())
        .collect()
}

/// `compute_disallowed_cops`: `DisallowedCops` names the ones to object to and `AllowedCops` the ones
/// to let through, and the first of the two wins where both were configured.
fn compute_disallowed(named: &[String], allowed: &[String], disallowed: &[String]) -> Vec<String> {
    if disallowed.is_empty() {
        return named
            .iter()
            .filter(|cop| !allowed.contains(cop))
            .cloned()
            .collect();
    }
    // A directive that switches off everything is objected to whole, since what it covers cannot be
    // narrowed down to the names that were configured.
    if named.iter().any(|cop| cop == "all") {
        return named.to_vec();
    }
    let mut unique: Vec<String> = Vec::new();
    for cop in named {
        if disallowed.contains(cop) && !unique.contains(cop) {
            unique.push(cop.clone());
        }
    }
    unique
}

/// The offense, whose correction drops the directive -- or the one name it objects to, where the
/// others were let through.
fn offense(
    comment: &Range<usize>,
    named: &[String],
    disallowed: &[String],
    configured: bool,
    context: &RuleContext<'_>,
) -> Offense {
    let message = match configured {
        true => format!(
            "RuboCop disable/enable directives for `{}` are not permitted.",
            disallowed.join("`, `")
        ),
        false => MSG.to_owned(),
    };
    let replacement = match named.len() == disallowed.len() {
        true => String::new(),
        false => without_disallowed(context.source.slice(comment.clone()), disallowed),
    };
    context
        .offense(message, comment.clone())
        .corrected_by(Edit {
            start: comment.start,
            end: comment.end,
            replacement,
            safe: true,
        })
}

/// `comment.text.sub(/#{Regexp.union(disallowed_cops)},?\s*/, '').sub(/,\s*$/, '')`: the first of the
/// objected-to names is taken out, and a comma the removal left at the end goes with it.
fn without_disallowed(text: &str, disallowed: &[String]) -> String {
    let Some((start, name)) = disallowed
        .iter()
        .filter_map(|name| text.find(name.as_str()).map(|start| (start, name)))
        .min_by_key(|(start, _)| *start)
    else {
        return trimmed_comma(text.to_owned());
    };
    let mut end = start + name.len();
    if text[end..].starts_with(',') {
        end += 1;
    }
    end += text[end..].len() - text[end..].trim_start().len();
    trimmed_comma(format!("{}{}", &text[..start], &text[end..]))
}

/// `.sub(/,\s*$/, '')`.
fn trimmed_comma(text: String) -> String {
    let trimmed = text.trim_end();
    match trimmed.strip_suffix(',') {
        Some(rest) => rest.to_owned(),
        None => text,
    }
}
