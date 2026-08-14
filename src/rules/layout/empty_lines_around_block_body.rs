//! `Layout/EmptyLinesAroundBlockBody`.

use super::empty_lines_around_body::{Target, body_container, body_of, check as check_body};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "no_empty_lines".to_owned());
    let mut targets = Vec::new();
    for node in context.nodes_of_any(&["block", "do_block"]) {
        let Some(parent) = node.parent_of(context) else {
            continue;
        };
        // `node.send_node.last_line`: everything of the call but the block itself. A stabby lambda's
        // send is the `->` alone.
        let first_line = match parent.kind_str() {
            "lambda" => parent.start_position().row + 1,
            _ => node
                .prev_sibling()
                .map_or(parent.start_position().row, |previous| {
                    previous.end_position().row
                })
                + 1,
        };
        // A block node upstream spans the call it hangs off, so it starts where the receiver does.
        targets.push(Target {
            first_line,
            last_line: node.end_position().row + 1,
            // `BlockNode#single_line?` compares the braces rather than the whole expression, so a
            // block opened on the last line of a multiline receiver counts as single-line.
            single_line: node.start_position().row == node.end_position().row,
            body: body_of(body_container(node)),
        });
    }
    check_body(context, "block", &style, targets, offenses);
}
