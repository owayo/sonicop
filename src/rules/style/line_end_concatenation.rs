//! Two string literals joined across a line break need no operator at all.
//!
//! This is the one cop here that reads the lexer's stream rather than the syntax tree: what it is
//! after is three tokens in a row -- a string, a `+` or `<<`, and a string on the next line -- and
//! the tree has already folded the pair into a single node by the time a cop sees it. The stream
//! comes from `layout::tokens`, which the Layout cops built for the same reason.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::layout::tokens::{Token, TokenKind, tokens};

/// `QUOTE_DELIMITERS`: only a plain quoted string can lose its operator. A `%q()` or a heredoc
/// cannot be continued with a backslash.
const QUOTE_DELIMITERS: [&str; 2] = ["'", "\""];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let tokens = tokens(context);
    for index in 0..tokens.len() {
        let Some(operator) = check_token_set(context, tokens, index) else {
            continue;
        };
        let text = context.source.slice(tokens[operator].range.clone());
        offenses.push(
            context
                .offense(
                    format!("Use `\\` instead of `{text}` to concatenate multiline strings."),
                    tokens[operator].range.clone(),
                )
                .corrected_by(correct(context, tokens[operator].range.clone())),
        );
    }
}

/// `check_token_set`: the index of the operator to report, when the three tokens at `index` are a
/// concatenation split across lines.
fn check_token_set(context: &RuleContext<'_>, tokens: &[Token], index: usize) -> Option<usize> {
    let (predecessor, operator, successor) = (
        tokens.get(index)?,
        tokens.get(index + 1)?,
        tokens.get(index + 2)?,
    );
    if !standard_string_literal(context, successor)
        || !matches!(operator.kind, TokenKind::Plus | TokenKind::LeftShift)
        || !standard_string_literal(context, predecessor)
    {
        return None;
    }
    // A concatenation that stays on one line is what `Style/StringConcatenation` is for.
    if operator.line == successor.line {
        return None;
    }
    // `eligible_next_successor?`: an operator binding tighter than the concatenation would take
    // the second string alone, and the backslash form would change what it applies to.
    let next_successor = token_after_last_string(tokens, successor, index);
    match next_successor.is_some_and(|token| {
        matches!(
            tokens[token].kind,
            TokenKind::Star | TokenKind::Percent | TokenKind::Dot | TokenKind::IndexBracket
        )
    }) {
        true => None,
        false => Some(index + 1),
    }
}

/// `token_after_last_string`: what follows the second string, which is the second string's closing
/// delimiter away when the string was spelled out rather than lexed whole.
fn token_after_last_string(tokens: &[Token], successor: &Token, base: usize) -> Option<usize> {
    let mut index = base + 3;
    if successor.kind == TokenKind::StringBegin {
        // A string spelled out may hold another through an interpolation, so the closing delimiter
        // that ends this one is the one the count comes back to zero on.
        let mut ends_to_find = 1;
        while ends_to_find > 0 {
            match tokens.get(index)?.kind {
                TokenKind::StringBegin => ends_to_find += 1,
                TokenKind::StringEnd => ends_to_find -= 1,
                _ => {}
            }
            index += 1;
        }
    }
    (index < tokens.len()).then_some(index)
}

/// `standard_string_literal?`: a string the lexer produced whole, or a delimiter of one written
/// with an ordinary quote.
fn standard_string_literal(context: &RuleContext<'_>, token: &Token) -> bool {
    match token.kind {
        TokenKind::String => true,
        TokenKind::StringBegin | TokenKind::StringEnd => {
            QUOTE_DELIMITERS.contains(&context.source.slice(token.range.clone()))
        }
        _ => false,
    }
}

/// The operator, the blanks after it, and the line continuation it may already carry, all of which
/// the backslash replaces.
fn correct(context: &RuleContext<'_>, operator: Range<usize>) -> Edit {
    let text = context.source.text().as_bytes();
    let mut end = operator.end;
    while text
        .get(end)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        end += 1;
    }
    // Taking one more character in only lands on a backslash where the line already continues, and
    // writing a second one there would escape it.
    if text.get(end) == Some(&b'\\') {
        end += 1;
    }
    Edit {
        start: operator.start,
        end,
        replacement: "\\".to_owned(),
        // `SafeAutoCorrect: false`: the receiver of a `<<` need not be a string, and `\` would
        // leave a syntax error behind where it is not.
        safe: false,
    }
}
