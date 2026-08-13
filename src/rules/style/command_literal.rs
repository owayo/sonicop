use std::collections::HashMap;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_USE_BACKTICKS: &str = "Use backticks around command string.";
const MSG_USE_PERCENT_X: &str = "Use `%x` around command string.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "backticks".to_owned());
    let allow_inner_backticks = context
        .setting::<bool>("AllowInnerBackticks")
        .unwrap_or(false);

    for node in context.nodes_of("subshell") {
        // `node.heredoc?`: a `<<`CMD`` opens a heredoc, which the grammar spells as its own node.
        let (Some(open), Some(close)) = (
            node.child(0),
            node.child(node.child_count().saturating_sub(1) as u32),
        ) else {
            continue;
        };
        if open.end_byte() > close.start_byte() {
            continue;
        }
        let body = context.source.slice(open.end_byte()..close.start_byte());
        let backtick_literal = context.source.node_text(open) == "`";
        // `contains_disallowed_backtick?`.
        let disallowed = !allow_inner_backticks && body.contains('`');
        let multiline = node.start_position().row != node.end_position().row;
        let (allowed, message) = match backtick_literal {
            true => (
                match style.as_str() {
                    "backticks" => !disallowed,
                    "mixed" => !multiline && !disallowed,
                    // `allowed_backtick_literal?` has no `percent_x` branch, so the `case` returns
                    // nil there and the literal is never allowed.
                    _ => false,
                },
                MSG_USE_PERCENT_X,
            ),
            false => (
                match style.as_str() {
                    "backticks" => disallowed,
                    "mixed" => multiline || disallowed,
                    "percent_x" => true,
                    _ => false,
                },
                MSG_USE_BACKTICKS,
            ),
        };
        if allowed {
            continue;
        }
        let offense = context.offense(message, node.byte_range());
        // `autocorrect` bails on a backtick anywhere in the body whatever the styles say, leaving
        // the offense with an empty corrector and so uncorrectable.
        if body.contains('`') {
            offenses.push(offense);
            continue;
        }
        let (opening, closing) = match backtick_literal {
            true => {
                let delimiter = preferred_delimiter(context);
                let mut characters = delimiter.chars();
                (
                    format!("%x{}", characters.next().unwrap_or_default()),
                    characters.next().map(String::from).unwrap_or_default(),
                )
            }
            false => ("`".to_owned(), "`".to_owned()),
        };
        offenses.push(offense.corrected_by_all([
            Edit {
                start: open.start_byte(),
                end: open.end_byte(),
                replacement: opening,
                safe: true,
            },
            Edit {
                start: close.start_byte(),
                end: close.end_byte(),
                replacement: closing,
                safe: true,
            },
        ]));
    }
}

/// `preferred_delimiter`: what `Style/PercentLiteralDelimiters` would write around a `%x`.
fn preferred_delimiter(context: &RuleContext<'_>) -> String {
    let configured = context
        .setting_of::<HashMap<String, String>>(
            "Style/PercentLiteralDelimiters",
            "PreferredDelimiters",
        )
        .unwrap_or_default();
    configured
        .get("%x")
        .or_else(|| configured.get("default"))
        .cloned()
        .unwrap_or_else(|| "()".to_owned())
}
