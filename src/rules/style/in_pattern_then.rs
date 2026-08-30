//! `Style/InPatternThen`: a one-line `in` clause says `then`, not `;`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 2.7`: pattern matching arrived in 2.7.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of("in_clause") {
        // `node.multiline?`, `node.then?` and `!node.body`: the semicolon only stands out on a
        // clause that fits on one line and carries a body.
        if node.start_position().row != node.end_position().row {
            continue;
        }
        let (Some(pattern), Some(body)) = (node.field("pattern"), node.field("body")) else {
            continue;
        };
        if super::nodes::children_in(body, context).is_empty() {
            continue;
        }
        let Some(separator) = super::conditional::token(body, &[";"]) else {
            continue;
        };
        // `node.then?`: upstream reports `node.loc.begin`, which is the token **between the
        // pattern and the body** -- the `;` or the `then`. A `;` further in is a statement
        // separator and none of this cop's business.
        //
        // `in b then c; d` already says `then`, so upstream returns. Taking any `;` inside the
        // body turns that into `in b then c then d`, which is a different program.
        if separator.start_byte() != body.start_byte() {
            continue;
        }
        let message = format!(
            "Do not use `in {0};`. Use `in {0} then` instead.",
            pattern_source(pattern, context)
        );
        offenses.push(
            context
                .offense(message, separator.byte_range())
                .corrected_by(Edit {
                    start: separator.start_byte(),
                    end: separator.end_byte(),
                    replacement: " then".to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `alternative_pattern_source`: an alternation is spelled back with one space around each `|`,
/// whatever it was written with.
fn pattern_source(pattern: Node<'_>, context: &RuleContext<'_>) -> String {
    if pattern.kind_str() != "alternative_pattern" {
        return context.source.node_text(pattern).to_owned();
    }
    let mut parts = Vec::new();
    collect(pattern, context, &mut parts);
    parts.join(" | ")
}

fn collect(pattern: Node<'_>, context: &RuleContext<'_>, parts: &mut Vec<String>) {
    for child in super::nodes::children_in(pattern, context) {
        if child.kind_str() == "alternative_pattern" {
            collect(child, context, parts);
        } else {
            parts.push(context.source.node_text(child).to_owned());
        }
    }
}
