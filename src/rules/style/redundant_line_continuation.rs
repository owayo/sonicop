//! `Style/RedundantLineContinuation`: a `\` at the end of a line the parser did not need.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::layout::tokens::{Token, TokenKind, tokens};
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

const MSG: &str = "Redundant line continuation.";

/// The node kinds that open a literal: everything below one of these is text rather than code,
/// which is what `within_string_content?` reads off the token stream.
const LITERALS: &[&str] = &[
    "string",
    "bare_string",
    "bare_symbol",
    "delimited_symbol",
    "regex",
    "subshell",
    "heredoc_body",
    "string_array",
    "symbol_array",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(ast) = ast_range(context) else {
        return;
    };
    let strings = string_content_ranges(context);
    let candidates: Vec<Range<usize>> = line_continuations(context, &ast)
        .into_iter()
        .filter(|range| {
            !within_comment(context, range)
                && !implicit_string_concatenation(context, range)
                && !overlaps_any(&strings, range)
                && !leading_dot_method_chain_with_blank_line(context, range)
                && !statement_would_end(context, range)
        })
        .collect();
    let verified = crate::rules::support::verified_by_reparse(
        context,
        candidates,
        |range| vec![remove_first(range)],
        Clone::clone,
        crate::rules::support::Verification {
            // `oversized: :verify`: the reparse is the whole test, so a scope too large to
            // reparse cannot be accepted unverified.
            verify_oversized: true,
            ..Default::default()
        },
    );
    for range in verified {
        offenses.push(
            context
                .offense(MSG, range.clone())
                .corrected_by(remove_first(&range)),
        );
    }
    inspect_end_of_ruby_code_line_continuation(context, &ast, offenses);
}

/// `processed_source.ast.source_range`: the span the parsed statements cover, which stops before a
/// continuation written after the last of them.
fn ast_range(context: &RuleContext<'_>) -> Option<Range<usize>> {
    let root = context.root_node();
    let _cursor = root.walk();
    let statements: Vec<Node<'_>> = named_children_of(root, context)
        .into_iter()
        // `__END__` and what follows it is `DATA`, not code: upstream's AST stops at the keyword,
        // so a `\` down there is out of the range the candidates are searched in.
        .filter(|child| !matches!(child.kind_str(), "comment" | "uninterpreted"))
        .collect();
    let (first, last) = (statements.first()?, statements.last()?);
    Some(first.start_byte()..last.end_byte())
}

/// `each_match_range(range, /(\\\n)/)`.
fn line_continuations(context: &RuleContext<'_>, ast: &Range<usize>) -> Vec<Range<usize>> {
    let source = context.source.text().as_bytes();
    let mut found = Vec::new();
    let mut offset = ast.start;
    while offset + 1 < ast.end.min(source.len()) {
        if source[offset] == b'\\' && source[offset + 1] == b'\n' {
            found.push(offset..offset + 2);
            offset += 2;
            continue;
        }
        offset += 1;
    }
    found
}

/// `within_comment?`.
fn within_comment(context: &RuleContext<'_>, range: &Range<usize>) -> bool {
    context
        .comment_ranges()
        .iter()
        .any(|comment| comment.start < range.end && range.start < comment.end)
}

/// `implicit_string_concatenation?`: two string literals written one under the other, which the
/// continuation is what joins.
fn implicit_string_concatenation(context: &RuleContext<'_>, range: &Range<usize>) -> bool {
    let stream: &[Token] = tokens(context);
    let line = context.source.line_column(range.start).0;
    let before = stream.iter().rfind(|token| token.range.end <= range.start);
    let ends = before.is_some_and(|token| {
        matches!(token.kind, TokenKind::String | TokenKind::StringEnd)
            && context
                .source
                .line_column(token.range.end.saturating_sub(1))
                .0
                == line
    });
    if !ends {
        return false;
    }
    stream
        .iter()
        .find(|token| token.range.start >= range.end)
        .is_some_and(|token| {
            matches!(token.kind, TokenKind::String | TokenKind::StringBegin)
                && token.line == line + 1
        })
}

/// `within_string_content?`: what the lexer hands out as string text rather than as code.
fn string_content_ranges(context: &RuleContext<'_>) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut stack = vec![context.root_node()];
    while let Some(node) = stack.pop() {
        if !LITERALS.contains(&node.kind_str()) {
            crate::rules::push_named_children_in(node, context, &mut stack);
            continue;
        }
        // What an interpolation holds is code again, so it is cut out of the literal and walked.
        let interpolations = interpolations_in(node);
        found.extend(subtract(node.byte_range(), &interpolations));
        stack.extend(interpolations.iter().map(|(_, node)| *node));
    }
    found
}

fn interpolations_in<'tree>(node: Node<'tree>) -> Vec<(Range<usize>, Node<'tree>)> {
    let mut found = Vec::new();
    let mut stack: Vec<Node<'tree>> = Vec::new();
    crate::rules::push_named_children(node, &mut stack);
    while let Some(child) = stack.pop() {
        if child.kind_str() == "interpolation" {
            found.push((child.byte_range(), child));
            continue;
        }
        crate::rules::push_named_children(child, &mut stack);
    }
    found
}

/// The parts of `range` that no hole covers.
fn subtract(range: Range<usize>, holes: &[(Range<usize>, Node<'_>)]) -> Vec<Range<usize>> {
    let mut cuts: Vec<Range<usize>> = holes.iter().map(|(hole, _)| hole.clone()).collect();
    cuts.sort_by_key(|hole| hole.start);
    let mut found = Vec::new();
    let mut start = range.start;
    for hole in cuts {
        if hole.start > start {
            found.push(start..hole.start);
        }
        start = start.max(hole.end);
    }
    if start < range.end {
        found.push(start..range.end);
    }
    found
}

fn overlaps_any(ranges: &[Range<usize>], range: &Range<usize>) -> bool {
    ranges
        .iter()
        .any(|other| other.start < range.end && range.start < other.end)
}

/// `leading_dot_method_chain_with_blank_line?`.
fn leading_dot_method_chain_with_blank_line(
    context: &RuleContext<'_>,
    range: &Range<usize>,
) -> bool {
    let line = context.source.line_column(range.start).0;
    let text = context.source.line(line);
    let trimmed = text.trim();
    if !(trimmed.starts_with('.') || trimmed.starts_with("&.")) {
        return false;
    }
    context.source.line(line + 1).trim().is_empty()
}

/// Whether removing the continuation would end the statement where the next line cannot pick it
/// up again.
///
/// `verified_by_reparse` cannot always tell: this grammar joins the two lines whether the
/// backslash is there or not, so a source RuboCop's parser rejects can still reparse cleanly here.
/// What decides it is what stands on either side of the break.
fn statement_would_end(context: &RuleContext<'_>, range: &Range<usize>) -> bool {
    let line = context.source.line_column(range.start).0;
    let before = context
        .source
        .slice(context.source.line_start(line)..range.start);
    let before = before.trim_end();
    if before.is_empty() || !ends_an_expression(before) {
        return false;
    }
    let next = context.source.line(line + 1);
    let next = next.trim();
    // A leading `.` on the very next line is the one thing that still continues the expression.
    if next.starts_with('.') || next.starts_with("&.") {
        return false;
    }
    // A blank line is joined to the line above and contributes nothing of its own, so what decides
    // the question is the first line with something on it. The test below reads an empty next line
    // as "opens nothing, so the statement ends here" and suppressed the offense, which is the
    // opposite of what emptiness means for everything except a chain.
    //
    // The exception is a leading `.` after the blank line, and it is the backslash that makes it
    // work: Ruby joins a leading-dot line to the one above it, but **a blank line breaks that**
    // (`ruby` answers `unexpected '.', ignoring it`). So the continuation is carrying the chain
    // across the gap and removing it would change the program -- upstream reports no offense for
    // `r = foo \` / `` / `  .bar`, however many blank lines sit in between.
    if next.is_empty() {
        let following = (line + 1..=context.source.line_count())
            .map(|number| context.source.line(number))
            .find(|text| !text.trim().is_empty());
        return following.is_some_and(|text| {
            let text = text.trim_start();
            text.starts_with('.') || text.starts_with("&.")
        });
    }
    // Everything else the next line could open would have joined the expression, which is what
    // the backslash was doing; only a closing keyword or bracket stands on its own.
    !only_closes_something(next)
}

/// Whether the text stands as an expression of its own, rather than asking for more.
fn ends_an_expression(text: &str) -> bool {
    let last = text.chars().next_back().unwrap_or(' ');
    if "+-*/%=<>&|^~([{.:,".contains(last) {
        return false;
    }
    let word = text
        .rsplit(|character: char| !(character.is_alphanumeric() || character == '_'))
        .next()
        .unwrap_or("");
    !matches!(
        word,
        "and"
            | "or"
            | "not"
            | "if"
            | "unless"
            | "while"
            | "until"
            | "then"
            | "do"
            | "else"
            | "elsif"
            | "in"
            | "when"
            | "case"
            | "rescue"
            | "ensure"
            | "begin"
            | "return"
            | "yield"
            | "class"
            | "module"
            | "def"
    )
}

/// Whether the line can do nothing but close what was opened before it, which is the one thing
/// that may follow a statement the backslash no longer joins.
fn only_closes_something(text: &str) -> bool {
    let first = text.chars().next().unwrap_or(' ');
    if ")]}".contains(first) {
        return true;
    }
    let word = text
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .next()
        .unwrap_or("");
    matches!(
        word,
        "end" | "else" | "elsif" | "when" | "in" | "rescue" | "ensure" | "then" | "do"
    )
}

/// `inspect_end_of_ruby_code_line_continuation`.
fn inspect_end_of_ruby_code_line_continuation(
    context: &RuleContext<'_>,
    ast: &Range<usize>,
    offenses: &mut Vec<Offense>,
) {
    let last_line = context.source.line_column(ast.end).0;
    let text = context.source.line(last_line);
    if !text.trim_end_matches(['\r', '\n']).ends_with('\\') {
        return;
    }
    let line_range = context.source.line_range(last_line);
    let end = match context.source.slice(line_range.clone()).ends_with('\n') {
        true => line_range.end - 1,
        false => line_range.end,
    };
    let range = end.saturating_sub(1)..end;
    if within_comment(context, &range) {
        return;
    }
    let verified = crate::rules::support::verified_by_reparse(
        context,
        vec![range.clone()],
        |range| vec![remove_first(range)],
        Clone::clone,
        crate::rules::support::Verification {
            // `oversized: :verify`: the reparse is the whole test, so a scope too large to
            // reparse cannot be accepted unverified.
            verify_oversized: true,
            ..Default::default()
        },
    );
    if verified.is_empty() {
        return;
    }
    offenses.push(
        context
            .offense(MSG, range.clone())
            .corrected_by(remove_first(&range)),
    );
}

/// `corrector.remove_leading(range, 1)`.
fn remove_first(range: &Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.start + 1,
        replacement: String::new(),
        safe: true,
    }
}
