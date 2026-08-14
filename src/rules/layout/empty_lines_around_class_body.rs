//! `Layout/EmptyLinesAroundClassBody`.

use super::empty_lines_around_body::{Target, body_container, body_of, check as check_body};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "no_empty_lines".to_owned());
    let mut targets = Vec::new();
    for node in context.nodes_of_any(&["class", "singleton_class"]) {
        // `adjusted_first_line`: a superclass written over several lines pushes the body's opening
        // down with it.
        let first_line = node
            .field("superclass")
            .and_then(|superclass| superclass.named_child(0))
            .map_or(node.start_position().row + 1, |parent_class| {
                parent_class.end_position().row + 1
            });
        targets.push(Target {
            first_line,
            last_line: node.end_position().row + 1,
            single_line: node.start_position().row == node.end_position().row,
            body: body_of(body_container(node)),
        });
    }
    check_body(context, "class", &style, targets, offenses);
}
