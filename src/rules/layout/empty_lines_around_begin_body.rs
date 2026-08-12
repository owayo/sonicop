//! `Layout/EmptyLinesAroundBeginBody`.

use super::empty_lines_around_body::{Body, Target, check as check_body};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // The cop hands the mixin no body at all, so only the lines framing the `begin` matter.
    let targets = context
        .nodes_of("begin")
        .map(|node| Target {
            first_line: node.start_position().row + 1,
            last_line: node.end_position().row + 1,
            single_line: node.start_position().row == node.end_position().row,
            body: Body::None,
        })
        .collect();
    check_body(context, "`begin`", "no_empty_lines", targets, offenses);
}
