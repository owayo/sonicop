//! `Layout/SpaceBeforeComment`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::comments;

const MSG: &str = "Put a space before an end-of-line comment.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for comment in comments(context) {
        // `token1.pos.end == token2.pos.begin` over consecutive tokens: whatever came before the
        // comment ends exactly where it starts, which is the same as saying that the character
        // before it belongs to a token rather than to the whitespace between two.
        let Some(previous) = text[..comment.start].chars().next_back() else {
            continue;
        };
        if previous.is_whitespace() {
            continue;
        }
        offenses.push(context.offense(MSG, comment.clone()).corrected_by(Edit {
            start: comment.start,
            end: comment.start,
            replacement: " ".to_owned(),
            safe: true,
        }));
    }
}
