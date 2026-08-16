//! `Style/MultilineInPatternThen`: an `in` clause whose body is on the next line needs no `then`.

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::final_pos;

const MSG: &str = "Do not use `then` for multiline `in` statement.";

/// `minimum_target_ruby_version 2.7`: pattern matching arrived in 2.7.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of("in_clause") {
        let (Some(pattern), Some(body)) = (node.field("pattern"), node.field("body")) else {
            continue;
        };
        let Some(keyword) = super::conditional::token(body, &["then"]) else {
            continue;
        };
        // `require_then?`: a pattern spread over several lines needs the keyword to close it, and
        // so does a body written beside the `in`.
        if pattern.start_position().row != pattern.end_position().row {
            continue;
        }
        if super::nodes::children(body)
            .first()
            .is_some_and(|first| first.start_position().row == node.start_position().row)
        {
            continue;
        }
        // `range_with_surrounding_space(side: :left, newlines: false)`: the blank in front of the
        // keyword goes with it, but the line break before that stays.
        let start = final_pos(
            context.source.text(),
            keyword.start_byte(),
            false, false,
            false,
            false,
        );
        offenses.push(
            context
                .offense(MSG, keyword.byte_range())
                .corrected_by(Edit {
                    start,
                    end: keyword.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
