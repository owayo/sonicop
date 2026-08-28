//! `Layout/InitialIndentation`.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Indentation of first line in file detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(token) = first_token(context) else {
        return;
    };
    if context.source.line_column(token.start).1 == 1 {
        return;
    }
    let text = context.source.text();
    let mut start = token.start;
    while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    // A byte order mark also puts the first token past column zero, and there is nothing to remove
    // in front of it.
    if start == token.start {
        return;
    }
    offenses.push(context.offense(MSG, token.clone()).corrected_by(Edit {
        start,
        end: token.start,
        replacement: String::new(),
        safe: true,
    }));
}

/// `processed_source.tokens.find { |t| !t.text.start_with?('#') }`: comments are tokens now, so the
/// first token is whatever the file opens with other than a `#` comment -- a `=begin` block is one.
fn first_token(context: &RuleContext<'_>) -> Option<Range<usize>> {
    let text = context.source.text();
    // A byte order mark is not whitespace to `trim_start`, and it is no token to the lexer either:
    // left in, the search settles on the mark itself and never reaches the first real token.
    let mut offset = match text.starts_with('\u{feff}') {
        true => '\u{feff}'.len_utf8(),
        false => 0,
    };
    // The lexer stops at `__END__`, so nothing past it is a token to find.
    let limit = crate::rules::support::code_before_data(context).len();
    loop {
        let rest = &text[offset..];
        let blanks = rest.len() - rest.trim_start().len();
        offset += blanks;
        if offset >= limit {
            return None;
        }
        match context
            .comment_ranges()
            .iter()
            .find(|comment| comment.start == offset)
        {
            Some(comment) if text[comment.clone()].starts_with('#') => offset = comment.end,
            Some(comment) => return Some(comment.clone()),
            None => {
                // `processed_source.tokens` holds lexer tokens, so the offense covers the first
                // **token** -- `puts`, not the call it opens. Walking to the leaf gives the same
                // span.
                let mut node = context
                    .root_node()
                    .descendant_for_byte_range(offset, offset + 1)?;
                while let Some(first) = node.child(0) {
                    node = first;
                }
                return Some(node.byte_range());
            }
        }
    }
}
