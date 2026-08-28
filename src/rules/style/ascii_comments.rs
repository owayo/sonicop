use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Use only ascii symbols in comments.";

/// The first run of non-ASCII characters in each comment, unless every non-ASCII character it holds
/// is one of `AllowedChars`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = context
        .setting::<Vec<String>>("AllowedChars")
        .unwrap_or_default();
    for range in context.comment_ranges() {
        let range = super::comments::parser_range(range, context);
        // The parser's span for a block comment can run past a file with no final newline.
        let range = range.start..range.end.min(context.source.text().len());
        let comment = &context.source.text()[range.clone()];
        if comment.is_ascii() {
            continue;
        }
        // `only_allowed_non_ascii_chars?`: every non-ASCII character is allowed, one at a time.
        if comment
            .chars()
            .filter(|character| !character.is_ascii())
            .all(|character| allowed.iter().any(|entry| entry == &character.to_string()))
        {
            continue;
        }
        let Some((start, length)) = first_non_ascii_run(comment) else {
            continue;
        };
        let start = range.start + start;
        offenses.push(context.offense(MSG, start..start + length));
    }
}

/// `/[^[:ascii:]]+/`: the offset and length of the first run, in bytes.
fn first_non_ascii_run(comment: &str) -> Option<(usize, usize)> {
    let start = comment
        .char_indices()
        .find(|(_, character)| !character.is_ascii())
        .map(|(offset, _)| offset)?;
    let length = comment[start..]
        .char_indices()
        .find(|(_, character)| character.is_ascii())
        .map_or(comment.len() - start, |(offset, _)| offset);
    Some((start, length))
}
