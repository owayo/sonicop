//! `Layout/ExtraSpacing`.
//!
//! The cop walks the file's tokens in pairs and reports the space between two neighbours on one
//! line whenever more than one character separates them. Everything else it does is a way of
//! letting deliberate padding through: a run of spaces that lines code up with the line above or
//! below is excused by `AllowForAlignment`, the space before a trailing comment by
//! `AllowBeforeTrailingComments`, and the gap between a key and its value in a multiline hash is
//! left to `Layout/HashAlignment`.

use std::cell::OnceCell;
use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::alignment::{Aligned, Alignment};
use super::support::{comments, hash_literals};
use super::tokens::{Token, tokens};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

const MSG_UNNECESSARY: &str = "Unnecessary spacing detected.";
const MSG_UNALIGNED_ASGN: &str = "`=` is not aligned with the preceding assignment.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `processed_source.blank?`: a file the parser produced no program for has no tokens to pair
    // up, comments included.
    if is_blank(context) {
        return;
    }
    let settings = Settings::read(context);
    let stream: &[Token] = tokens(context);
    let aligned_comments = aligned_comment_lines(context);
    // Only a file that actually pads a token pays for the line and operator bookkeeping.
    let alignment = OnceCell::new();
    let ignored = OnceCell::new();
    // `@corrected`: one `=` is moved at most once, however many offenses ask for it.
    let mut corrected = HashSet::new();
    for pair in stream.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        let aligns_assignments = settings.force_equal_sign_alignment
            && alignment
                .get_or_init(|| Alignment::new(context))
                .assignment_token(right.line)
                == Some(&right.range);
        if aligns_assignments {
            check_assignment(context, &alignment, right, &mut corrected, offenses);
        } else if let Some(offense) = check_other(
            context,
            &settings,
            &alignment,
            &ignored,
            &aligned_comments,
            left,
            right,
        ) {
            offenses.push(offense);
        }
    }
}

struct Settings {
    allow_for_alignment: bool,
    allow_before_trailing_comments: bool,
    force_equal_sign_alignment: bool,
}

impl Settings {
    fn read(context: &RuleContext<'_>) -> Self {
        Self {
            allow_for_alignment: context.setting("AllowForAlignment").unwrap_or(true),
            allow_before_trailing_comments: context
                .setting("AllowBeforeTrailingComments")
                .unwrap_or(false),
            force_equal_sign_alignment: context.setting("ForceEqualSignAlignment").unwrap_or(false),
        }
    }
}

/// `check_other` and the `extra_space_range` it reports through.
fn check_other<'src>(
    context: &RuleContext<'src>,
    settings: &Settings,
    alignment: &OnceCell<Alignment<'src>>,
    ignored: &OnceCell<Vec<Range<usize>>>,
    aligned_comments: &HashSet<usize>,
    left: &Token,
    right: &Token,
) -> Option<Offense> {
    if settings.allow_before_trailing_comments
        && context.source.text()[right.range.clone()].starts_with('#')
    {
        return None;
    }
    if left.line != right.line {
        return None;
    }
    let start = left.range.end;
    // One space is what the pair is left with, so the last character of the run is not reported.
    // RuboCop addresses the buffer by character, so it is one character rather than one byte that
    // comes off -- the gap holds nothing but blanks, but a grammar laxer than Ruby's lexer can
    // still let a multibyte one through.
    let end = previous_character(context.source.text(), right.range.start)?;
    if end <= start {
        return None;
    }
    if settings.allow_for_alignment && aligned_token(context, alignment, aligned_comments, right) {
        return None;
    }
    let ignored = ignored.get_or_init(|| ignored_ranges(context));
    if ignored.iter().any(|range| range.contains(&start)) {
        return None;
    }
    Some(
        context
            .offense(MSG_UNNECESSARY, start..end)
            .corrected_by(Edit {
                start,
                end,
                replacement: String::new(),
                safe: true,
            }),
    )
}

/// `aligned_tok?`: a comment is aligned when another comment starts in its column, anything else
/// when the mixin finds something above or below it to line up with.
fn aligned_token<'src>(
    context: &RuleContext<'src>,
    alignment: &OnceCell<Alignment<'src>>,
    aligned_comments: &HashSet<usize>,
    token: &Token,
) -> bool {
    if token.is_comment() {
        return aligned_comments.contains(&token.line);
    }
    alignment
        .get_or_init(|| Alignment::new(context))
        .aligned_with_something(&token.range)
}

/// `check_assignment`, the `ForceEqualSignAlignment` path: an `=` that does not line up with the
/// nearest assignment above it moves, and takes the whole run of assignments with it.
fn check_assignment<'src>(
    context: &RuleContext<'src>,
    alignment: &OnceCell<Alignment<'src>>,
    token: &Token,
    corrected: &mut HashSet<usize>,
    offenses: &mut Vec<Offense>,
) {
    let alignment = alignment.get_or_init(|| Alignment::new(context));
    if alignment.aligned_with_preceding_equals(&token.range) != Aligned::No {
        return;
    }
    let edits = align_equal_signs(context, alignment, token.line, corrected);
    offenses.push(
        context
            .offense(MSG_UNALIGNED_ASGN, token.range.clone())
            .corrected_by_all(edits),
    );
}

/// `align_equal_signs`: every assignment of the run is padded out to the column the widest of them
/// would need once its own excess padding is gone.
fn align_equal_signs(
    context: &RuleContext<'_>,
    alignment: &Alignment<'_>,
    line: usize,
    corrected: &mut HashSet<usize>,
) -> Vec<Edit> {
    let operators: Vec<(usize, Range<usize>)> = alignment
        .all_relevant_assignment_lines(line)
        .into_iter()
        .filter_map(|line| Some((line, alignment.assignment_token(line)?.clone())))
        .collect();
    let Some(align_to) = operators
        .iter()
        .map(|(line, operator)| align_column(context, alignment, *line, operator))
        .max()
    else {
        return Vec::new();
    };
    let mut edits = Vec::new();
    for (_, operator) in operators {
        if !corrected.insert(operator.start) {
            continue;
        }
        let last_column = context.source.line_column(operator.end).1 as i64 - 1;
        let delta = align_to - last_column;
        if delta > 0 {
            edits.push(Edit {
                start: operator.start,
                end: operator.start,
                replacement: " ".repeat(usize::try_from(delta).unwrap_or(0)),
                safe: true,
            });
        } else if delta < 0 {
            // `remove_preceding`: the padding written before the operator is what shrinks.
            let width = usize::try_from(-delta).unwrap_or(0);
            edits.push(Edit {
                start: operator.start.saturating_sub(width),
                end: operator.start,
                replacement: String::new(),
                safe: true,
            });
        }
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));
    edits
}

/// `align_column`: the column this `=` would end in once the spaces written before it are gone.
fn align_column(
    context: &RuleContext<'_>,
    alignment: &Alignment<'_>,
    line: usize,
    operator: &Range<usize>,
) -> i64 {
    let column = context.source.line_column(operator.start).1 - 1;
    let text = alignment.line(line);
    let leading: String = text.chars().take(column).collect();
    let spaces = leading.chars().rev().take_while(|c| *c == ' ').count() as i64;
    let last_column = context.source.line_column(operator.end).1 as i64 - 1;
    last_column - spaces + 1
}

/// `aligned_locations(processed_source.comments.map(&:loc))`: the lines of every pair of
/// neighbouring comments that start in the same column.
fn aligned_comment_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    let mut aligned = HashSet::new();
    let comments = comments(context);
    for pair in comments.windows(2) {
        let (first_line, first_column) = context.source.line_column(pair[0].start);
        let (second_line, second_column) = context.source.line_column(pair[1].start);
        if first_column == second_column {
            aligned.insert(first_line);
            aligned.insert(second_line);
        }
    }
    aligned
}

/// `ignored_ranges`: the space between a key and its value in a multiline hash, which
/// `Layout/HashAlignment` owns.
fn ignored_ranges(context: &RuleContext<'_>) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for elements in hash_literals(context) {
        let Some(span) = hash_span(&elements, context) else {
            continue;
        };
        if context.source.line_column(span.start).0 == context.source.line_column(span.end).0 {
            continue;
        }
        for element in elements {
            let (Some(key), Some(value)) = (element.field("key"), element.field("value")) else {
                continue;
            };
            ranges.push(key.end_byte()..value.start_byte());
        }
    }
    ranges
}

/// Where the `hash` node upstream builds for these elements begins and ends. A braced literal is a
/// node of its own, whose braces count towards `single_line?`; a brace-less one is only the run of
/// pairs the parser folded together.
fn hash_span(elements: &[Node<'_>], context: &RuleContext<'_>) -> Option<Range<usize>> {
    let first = elements.first()?;
    let last = elements.last()?;
    match context.parent(*first).filter(|parent| parent.kind_str() == "hash") {
        Some(hash) => Some(hash.byte_range()),
        None => Some(first.start_byte()..last.end_byte()),
    }
}

/// Where the character before `offset` starts.
fn previous_character(text: &str, offset: usize) -> Option<usize> {
    let mut start = offset.checked_sub(1)?;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    Some(start)
}

/// `ProcessedSource#blank?`: the parser built no program, which is the case for a file holding
/// nothing but comments.
fn is_blank(context: &RuleContext<'_>) -> bool {
    let root = context.root_node();
    let _cursor = root.walk();
    !named_children_of(root, context)
        .into_iter()
        .any(|child| child.kind_str() != "comment")
}
