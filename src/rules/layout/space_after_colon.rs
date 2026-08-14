//! `Layout/SpaceAfterColon`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Space missing after colon.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("pair") {
        // `node.colon?`: a pair written with `=>` has no colon to look after, and a value-less
        // `{ x: }` is the shorthand rather than a missing space.
        if node.field("value").is_none() {
            continue;
        }
        if let Some(colon) = pair_colon(context, node) {
            report(context, colon, offenses);
        }
    }
    // `on_kwoptarg`: only a keyword parameter that was given a default.
    for node in context.nodes_of("keyword_parameter") {
        if node.field("value").is_none() {
            continue;
        }
        let Some(name) = node.field("name") else {
            continue;
        };
        // `node.loc.name.end.resize(1)`: the cop builds the colon's range from the name's end.
        report(context, name.end_byte()..(name.end_byte() + 1), offenses);
    }
}

/// `node.loc.operator`, when it is a `:` rather than a `=>`.
fn pair_colon(context: &RuleContext<'_>, node: Node<'_>) -> Option<Range<usize>> {
    let (key, value) = (
        node.field("key")?,
        node.field("value")?,
    );
    let text = context.source.text();
    let between = &text[key.end_byte()..value.start_byte()];
    let offset = between.len() - between.trim_start().len();
    let start = key.end_byte() + offset;
    text[start..].starts_with(':').then_some(start..(start + 1))
}

fn report(context: &RuleContext<'_>, colon: Range<usize>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    if colon.end > text.len() {
        return;
    }
    // `followed_by_space?`: a line break counts as space, the end of the file does not.
    if text.as_bytes()[colon.end..]
        .first()
        .is_some_and(u8::is_ascii_whitespace)
    {
        return;
    }
    offenses.push(context.offense(MSG, colon.clone()).corrected_by(Edit {
        start: colon.end,
        end: colon.end,
        replacement: " ".to_owned(),
        safe: true,
    }));
}
