//! `Layout/LineContinuationLeadingSpace`: which side of a line continuation the blank belongs to.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let leading_style = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "leading");
    let text = context.source.text();
    for node in context.nodes_of("chained_string") {
        if !context.source.node_text(node).contains('\\') {
            continue;
        }
        let first_line = node.start_position().row + 1;
        let last_line = node.end_position().row + 1;
        // `end_of_first_line` walks down the node's own lines, ending each round just past the
        // newline of the first line of the pair.
        let mut end_of_line = context.source.line_start(first_line);
        for line_number in first_line..last_line {
            let one = context.source.line(line_number);
            let two = context.source.line(line_number + 1);
            end_of_line += one.len();
            if !one.ends_with("\\\n") || covered_by_a_multiline_part(node, line_number) {
                continue;
            }
            let found = if leading_style {
                leading_offense(one, end_of_line, two)
            } else {
                trailing_offense(one, end_of_line, two)
            };
            let Some((range, insert_at)) = found else {
                continue;
            };
            let message = if leading_style {
                "Move trailing spaces to the start of the next line."
            } else {
                "Move leading spaces to the end of the previous line."
            };
            let spaces = text[range.clone()].to_owned();
            offenses.push(context.offense(message, range.clone()).corrected_by_all([
                Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                },
                Edit {
                    start: insert_at,
                    end: insert_at,
                    replacement: spaces,
                    safe: true,
                },
            ]));
        }
    }
}

/// `investigate_leading_style`: the blanks written before the closing quote of the first line.
fn leading_offense(one: &str, end_of_line: usize, two: &str) -> Option<(Range<usize>, usize)> {
    // `LINE_1_ENDING`: `['"]\s*\\\n`.
    let ending = line_one_ending(one)?;
    let before = &one[..one.len() - ending];
    let blanks = before
        .bytes()
        .rev()
        .take_while(u8::is_ascii_whitespace)
        .count();
    if blanks == 0 {
        return None;
    }
    let end = end_of_line - ending;
    // `LINE_2_BEGINNING`: the blanks and the quote the next line opens with.
    let insert_at = end_of_line + line_two_beginning(two)?;
    Some((end - blanks..end, insert_at))
}

/// `investigate_trailing_style`: the blanks written after the opening quote of the second line.
fn trailing_offense(one: &str, end_of_line: usize, two: &str) -> Option<(Range<usize>, usize)> {
    let beginning = line_two_beginning(two)?;
    let blanks = two[beginning..]
        .bytes()
        .take_while(u8::is_ascii_whitespace)
        .count();
    if blanks == 0 {
        return None;
    }
    let start = end_of_line + beginning;
    let insert_at = end_of_line - line_one_ending(one)?;
    Some((start..start + blanks, insert_at))
}

/// The length of `['"]\s*\\\n` at the end of the line, when it is there.
fn line_one_ending(line: &str) -> Option<usize> {
    let body = line.strip_suffix("\\\n")?;
    let blanks = body
        .bytes()
        .rev()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let quote = body.len().checked_sub(blanks + 1)?;
    matches!(body.as_bytes().get(quote), Some(b'\'') | Some(b'"')).then(|| line.len() - quote)
}

/// The length of `\A\s*['"]` at the start of the line, when it is there.
fn line_two_beginning(line: &str) -> Option<usize> {
    let blanks = line
        .bytes()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    matches!(line.as_bytes().get(blanks), Some(b'\'') | Some(b'"')).then_some(blanks + 1)
}

/// `node.children.none? { |c| (c.first_line...c.last_line).cover?(line_num) && c.multiline? }`.
fn covered_by_a_multiline_part(node: Node<'_>, line_number: usize) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|child| {
        let (first, last) = (child.start_position().row + 1, child.end_position().row + 1);
        first != last && (first..last).contains(&line_number)
    })
}
