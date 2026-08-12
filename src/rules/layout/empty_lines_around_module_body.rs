//! `Layout/EmptyLinesAroundModuleBody`.

use super::empty_lines_around_body::{Target, body_container, body_of, check as check_body};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "no_empty_lines".to_owned());
    let targets = context
        .nodes_of("module")
        .map(|node| Target {
            first_line: node.start_position().row + 1,
            last_line: node.end_position().row + 1,
            single_line: node.start_position().row == node.end_position().row,
            body: body_of(body_container(node)),
        })
        .collect();
    check_body(context, "module", &style, targets, offenses);
}
