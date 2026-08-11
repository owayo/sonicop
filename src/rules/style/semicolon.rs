use std::collections::HashSet;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::source::is_protected;

/// Node kinds whose named children are a sequence of statements.
///
/// RuboCop reaches the same set through the `begin` node its parser builds for any body holding
/// more than one expression. Restricting the scan to these kinds is what keeps `def foo; bar(1, 2);
/// end` quiet: an argument list also has two children ending on the line, but it is not a place
/// where a semicolon could be separating statements.
const STATEMENT_SEQUENCE_KINDS: &[&str] = &[
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    "ensure",
    "parenthesized_statements",
    "begin_block",
    "end_block",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    if !text.contains(';') {
        return;
    }
    let allow_as_expression_separator: bool = context
        .setting("AllowAsExpressionSeparator")
        .unwrap_or(false);

    let semicolons = semicolon_offsets(context);
    let mut reported: Vec<usize> = Vec::new();

    // RuboCop reports one semicolon per line here: the one that terminates the line or opens it.
    // A semicolon in the middle of a line is not an offense on its own -- `def foo; bar; end` is
    // left alone -- because that shape is a single expression, not two.
    for line_number in 1..=context.source.line_count() {
        if let Some(offset) = line_terminator_or_opener(context, line_number, &semicolons) {
            reported.push(offset);
        }
    }

    // The second pass is the one that makes `foo; bar` an offense: a line holding the end of more
    // than one statement really is separating expressions, and then *every* semicolon on it counts.
    if !allow_as_expression_separator {
        let separator_lines = expression_separator_lines(context);
        reported.extend(
            semicolons
                .iter()
                .filter(|(line, _)| separator_lines.contains(line))
                .map(|(_, offset)| *offset),
        );
    }

    reported.sort_unstable();
    reported.dedup();
    for offset in reported {
        // Only a semicolon with nothing but a comment after it can be dropped outright; removing
        // one that separates two expressions would join them into a single statement.
        let offense = context.offense(
            "Do not use semicolons to terminate expressions.",
            offset..offset + 1,
        );
        offenses.push(if trailing_on_line(context, offset) {
            offense.corrected_by(Edit {
                start: offset,
                end: offset + 1,
                replacement: String::new(),
                safe: true,
            })
        } else {
            offense
        });
    }
}

/// Every semicolon that is code rather than text, as `(line, byte offset)`.
fn semicolon_offsets(context: &RuleContext<'_>) -> Vec<(usize, usize)> {
    let ranges = context.protected_ranges();
    let text = context.source.text();
    text.bytes()
        .enumerate()
        .filter(|(offset, byte)| *byte == b';' && !is_protected(*offset, ranges))
        .map(|(offset, _)| (context.source.line_column(offset).0, offset))
        .collect()
}

/// The semicolon RuboCop reports for `line_number`, if any.
///
/// Upstream inspects the line's token list and accepts a single position: the last token, the
/// first token, or a semicolon hugging a closing or opening brace. Comments are not tokens, so a
/// trailing comment does not stop a line from ending in a semicolon.
fn line_terminator_or_opener(
    context: &RuleContext<'_>,
    line_number: usize,
    semicolons: &[(usize, usize)],
) -> Option<usize> {
    let on_line: Vec<usize> = semicolons
        .iter()
        .filter(|(line, _)| *line == line_number)
        .map(|(_, offset)| *offset)
        .collect();
    if on_line.is_empty() {
        return None;
    }

    let code = code_range(context, line_number)?;
    let text = context.source.text();
    let last = on_line[on_line.len() - 1];
    let first = on_line[0];

    if last + 1 == code.end {
        return Some(last);
    }
    if first == code.start {
        return Some(first);
    }
    // `foo { ; }` and `"#{ ; }"`: the semicolon sits against the brace that opens or closes the
    // block, which upstream treats the same as sitting at the edge of the line.
    let after_last = text[last + 1..code.end].trim();
    if after_last == "}" {
        return Some(last);
    }
    let before_first = text[code.start..first].trim_end();
    if before_first.ends_with('{') {
        return Some(first);
    }
    None
}

/// The byte range of `line_number` with comments and surrounding whitespace removed.
fn code_range(context: &RuleContext<'_>, line_number: usize) -> Option<std::ops::Range<usize>> {
    let text = context.source.text();
    let line = context.source.line_range(line_number);
    let comment_start = context
        .comment_ranges()
        .iter()
        .filter(|range| range.start >= line.start && range.start < line.end)
        .map(|range| range.start)
        .min()
        .unwrap_or(line.end);
    let slice = &text[line.start..comment_start];
    let start = line.start + (slice.len() - slice.trim_start().len());
    let end = line.start + slice.trim_end().len();
    (start < end).then_some(start..end)
}

/// Lines on which more than one statement ends, which is what makes a semicolon a separator.
fn expression_separator_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    let mut lines = HashSet::new();
    for node in context.nodes_of_any(STATEMENT_SEQUENCE_KINDS) {
        let mut cursor = node.walk();
        let mut ends: Vec<usize> = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() != "comment")
            .map(|child| child.end_position().row + 1)
            .collect();
        ends.sort_unstable();
        lines.extend(
            ends.windows(2)
                .filter(|pair| pair[0] == pair[1])
                .map(|pair| pair[0]),
        );
    }
    lines
}

/// Whether nothing but a comment follows the semicolon on its line.
fn trailing_on_line(context: &RuleContext<'_>, offset: usize) -> bool {
    let line_number = context.source.line_column(offset).0;
    code_range(context, line_number).is_none_or(|code| offset + 1 >= code.end)
}
