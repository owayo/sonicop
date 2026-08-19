//! `Layout/SpaceInsideParens`.
//!
//! `EnforcedStyle` picks between three readings of the space just inside a round bracket:
//! `no_space` forbids it, `space` requires it, and `compact` requires it except between two
//! brackets that sit next to each other.
//!
//! Upstream walks the lexer's token stream in neighbouring pairs rather than the syntax tree, and
//! this follows it, because the pair is what decides the case. A `(` whose partner is the very
//! next token is an empty pair and takes no space under any style; a pair split across two lines
//! has no space to speak of; and a pair whose second half is a comment is a line break in
//! disguise. Reading the source for brackets instead would also have to keep percent literals
//! out by hand -- `%w(a b)` holds no bracket the lexer ever saw -- and the token stream settles
//! that by construction.

use std::ops::Range;

use super::tokens::{Token, TokenKind, tokens};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Space inside parentheses detected.";
const MSG_SPACE: &str = "No space inside parentheses detected.";

#[derive(Clone, Copy, Eq, PartialEq)]
enum Style {
    NoSpace,
    Space,
    Compact,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = match context.setting::<String>("EnforcedStyle").as_deref() {
        Some("space") => Style::Space,
        Some("compact") => Style::Compact,
        _ => Style::NoSpace,
    };
    // `processed_source.sorted_tokens`, and the sort is load-bearing rather than defensive: the
    // stream puts a heredoc's body where its opener stands, so `foo(<<~A, bar)` hands over the
    // body before the comma and a pair taken off it would span the file backwards.
    let mut stream = tokens(context);
    stream.sort_by_key(|token| token.range.start);
    for pair in stream.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        match style {
            Style::NoSpace => correct_extraneous_space(context, first, second, offenses),
            Style::Space => {
                correct_extraneous_space_in_empty_parens(context, first, second, offenses);
                correct_missing_space(context, first, second, offenses);
            }
            Style::Compact => {
                correct_extraneous_space_in_empty_parens(context, first, second, offenses);
                if consecutive_parens(first, second) {
                    correct_extraneous_space_between_consecutive_parens(
                        context, first, second, offenses,
                    );
                } else {
                    correct_missing_space(context, first, second, offenses);
                }
            }
        }
    }
}

/// `correct_extraneous_space`: the `no_space` style, where any gap inside a bracket is reported.
fn correct_extraneous_space(
    context: &RuleContext<'_>,
    first: &Token,
    second: &Token,
    offenses: &mut Vec<Offense>,
) {
    if !parens(first, second) || second.is_comment() {
        return;
    }
    if first.line != second.line || !space_after(context, first) {
        return;
    }
    offenses.push(removal(context, first.range.end..second.range.start));
}

/// `correct_extraneous_space_between_consecutive_parens`: under `compact`, `( (` and `) )` close
/// up, but only when a single space separates them.
fn correct_extraneous_space_between_consecutive_parens(
    context: &RuleContext<'_>,
    first: &Token,
    second: &Token,
    offenses: &mut Vec<Offense>,
) {
    let range = first.range.end..second.range.start;
    if &context.source.text()[range.clone()] != " " {
        return;
    }
    offenses.push(removal(context, range));
}

/// `correct_extraneous_space_in_empty_parens`: an empty pair takes no space under any style that
/// asks for one.
fn correct_extraneous_space_in_empty_parens(
    context: &RuleContext<'_>,
    first: &Token,
    second: &Token,
    offenses: &mut Vec<Offense>,
) {
    if first.kind != TokenKind::LeftParenthesis || second.kind != TokenKind::RightParenthesis {
        return;
    }
    if empty_parens(context, first, second) {
        return;
    }
    offenses.push(removal(context, first.range.end..second.range.start));
}

/// `correct_missing_space`: the space `space` and `compact` require, reported on the character it
/// should precede.
fn correct_missing_space(
    context: &RuleContext<'_>,
    first: &Token,
    second: &Token,
    offenses: &mut Vec<Offense>,
) {
    if can_be_ignored(context, first, second) {
        return;
    }
    let range = if first.kind == TokenKind::LeftParenthesis {
        // `range_between(token2.begin_pos, token2.begin_pos + 1)`: upstream counts in characters,
        // so the range is the second token's first character rather than its first byte.
        let width = context.source.text()[second.range.clone()]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        second.range.start..(second.range.start + width)
    } else if second.kind == TokenKind::RightParenthesis {
        second.range.clone()
    } else {
        return;
    };
    offenses.push(
        context
            .offense(MSG_SPACE, range.clone())
            .corrected_by(Edit {
                start: range.start,
                end: range.start,
                replacement: " ".to_owned(),
                safe: true,
            }),
    );
}

/// `can_be_ignored?`.
fn can_be_ignored(context: &RuleContext<'_>, first: &Token, second: &Token) -> bool {
    if !parens(first, second) || empty_parens(context, first, second) || second.is_comment() {
        return true;
    }
    first.line != second.line || space_after(context, first)
}

/// `parens?`: the pair touches the inside of a bracket from one side or the other.
fn parens(first: &Token, second: &Token) -> bool {
    first.kind == TokenKind::LeftParenthesis || second.kind == TokenKind::RightParenthesis
}

/// `left_parens?` or `right_parens?`: two brackets facing the same way, which `compact` closes up.
fn consecutive_parens(first: &Token, second: &Token) -> bool {
    (first.kind == TokenKind::LeftParenthesis && second.kind == TokenKind::LeftParenthesis)
        || (first.kind == TokenKind::RightParenthesis && second.kind == TokenKind::RightParenthesis)
}

/// `range_between(token1.begin_pos, token2.end_pos).source == '()'`.
fn empty_parens(context: &RuleContext<'_>, first: &Token, second: &Token) -> bool {
    &context.source.text()[first.range.start..second.range.end] == "()"
}

/// `Token#space_after?`.
fn space_after(context: &RuleContext<'_>, token: &Token) -> bool {
    context
        .source
        .text()
        .as_bytes()
        .get(token.range.end)
        .is_some_and(u8::is_ascii_whitespace)
}

fn removal(context: &RuleContext<'_>, range: Range<usize>) -> Offense {
    context.offense(MSG, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    })
}
