//! `Layout/LeadingEmptyLines`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Unnecessary blank line at the beginning of the source.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    let start = text.len() - text.trim_start().len();
    if start >= text.len() {
        return;
    }
    // `processed_source.tokens[0]`: comments are tokens too, so a leading comment counts.
    let token = match context
        .comment_ranges()
        .iter()
        .find(|comment| comment.start == start)
    {
        Some(comment) => comment.clone(),
        None => match context.root_node().descendant_for_byte_range(start, start + 1) {
            Some(node) => node.byte_range(),
            None => return,
        },
    };
    if context.source.line_column(token.start).0 <= 1 {
        return;
    }
    offenses.push(context.offense(MSG, token.clone()).corrected_by(Edit {
        start: 0,
        end: token.start,
        replacement: String::new(),
        safe: true,
    }));
}
