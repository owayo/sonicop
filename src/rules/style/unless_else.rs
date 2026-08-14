use std::collections::HashSet;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not use `unless` with `else`. Rewrite these with the positive case first.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `ignore_node`: an `unless` reported inside another one is left uncorrected, because the
    // outer swap already moves the text this one would have rewritten.
    let mut ignored: HashSet<usize> = HashSet::new();
    for node in context.nodes_of("unless") {
        let Some(keyword) = node.child(0) else {
            continue;
        };
        let Some(alternative) = node.field("alternative") else {
            continue;
        };
        if alternative.kind_str() != "else" {
            continue;
        }
        let (Some(else_keyword), Some(end)) = (
            alternative.child(0),
            super::conditional::token(node, &["end"]),
        ) else {
            continue;
        };
        let inside_reported = std::iter::successors(node.parent_of(context), |current| current.parent_of(context))
            .any(|ancestor| ignored.contains(&ancestor.id()));
        let offense = context.offense(MSG, node.byte_range());
        if inside_reported {
            offenses.push(offense);
            continue;
        }
        ignored.insert(node.id());

        // `range_between_condition_and_else`: from the `then` if one was written, else from the
        // end of the condition, up to the `else` keyword.
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let body_start = node
            .field("consequence")
            .and_then(|consequence| super::conditional::token(consequence, &["then"]))
            .map_or_else(|| condition.end_byte(), |then| then.end_byte());
        let body = body_start..else_keyword.start_byte();
        let alternate = else_keyword.end_byte()..end.start_byte();
        let text = context.source.text();
        offenses.push(offense.corrected_by_all([
            Edit {
                start: keyword.start_byte(),
                end: keyword.end_byte(),
                replacement: "if".to_owned(),
                safe: true,
            },
            // `corrector.swap`: each half takes the other's text.
            Edit {
                start: body.start,
                end: body.end,
                replacement: text[alternate.clone()].to_owned(),
                safe: true,
            },
            Edit {
                start: alternate.start,
                end: alternate.end,
                replacement: text[body].to_owned(),
                safe: true,
            },
        ]));
    }
}
