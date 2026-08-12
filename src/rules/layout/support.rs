//! Scanning and node grouping shared by more than one Layout cop.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Edit;
use crate::rules::RuleContext;

/// The run of spaces and tabs ending at `offset`.
pub(super) fn whitespace_before(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}

/// The run of spaces and tabs starting at `offset`.
pub(super) fn whitespace_after(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut end = offset;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    offset..end
}

/// The hash literals of a file, each as the run of elements upstream's parser folds into one
/// `hash` node.
///
/// A braced hash is a node of its own here as well, but a brace-less one -- `foo(a: 1, b: 2)`,
/// `[a: 1]`, `foo[a: 1]` -- is not: the grammar leaves its pairs as siblings of whatever was
/// written before them, while upstream's parser wraps the trailing run of `key: value` pairs and
/// `**splat`s into a single `hash`. A cop written against `on_hash` has to see that run as one
/// literal or it measures alignment against the wrong first pair.
pub(super) fn hash_literals<'tree>(context: &'tree RuleContext<'tree>) -> Vec<Vec<Node<'tree>>> {
    let mut literals: Vec<(usize, Vec<Node<'tree>>)> = Vec::new();
    for node in context.nodes_of("hash") {
        let mut cursor = node.walk();
        let elements: Vec<Node<'tree>> = node
            .named_children(&mut cursor)
            .filter(|child| is_hash_element(*child))
            .collect();
        if !elements.is_empty() {
            literals.push((node.start_byte(), elements));
        }
    }
    for container in context.nodes_of_any(&["argument_list", "array", "element_reference"]) {
        let mut cursor = container.walk();
        // A comment written between two pairs is a node here and nothing at all upstream, so it
        // must not break the run it sits in.
        let children: Vec<Node<'tree>> = container
            .named_children(&mut cursor)
            .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
            .collect();
        let mut index = 0;
        while index < children.len() {
            if !is_hash_element(children[index]) {
                index += 1;
                continue;
            }
            let start = index;
            while index < children.len() && is_hash_element(children[index]) {
                index += 1;
            }
            literals.push((
                children[start].start_byte(),
                children[start..index].to_vec(),
            ));
        }
    }
    literals.sort_by_key(|(start, _)| *start);
    literals.into_iter().map(|(_, elements)| elements).collect()
}

fn is_hash_element(node: Node<'_>) -> bool {
    matches!(node.kind(), "pair" | "hash_splat_argument")
}

/// `Util.begins_its_line?`: the first non-blank character of the line is where the node starts.
pub(super) fn begins_its_line(context: &RuleContext<'_>, offset: usize) -> bool {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    context.source.text()[start..offset]
        .chars()
        .all(char::is_whitespace)
}

/// A set of `insert_before` and `remove` corrections over one node, collapsed into the single
/// replacement `Edit` carries.
pub(super) struct Edits<'a> {
    #[allow(dead_code)]
    text: &'a str,
    /// `(start, end, replacement)` triples, in the order they were recorded.
    parts: Vec<(usize, usize, String)>,
}

impl<'a> Edits<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        Self {
            text,
            parts: Vec::new(),
        }
    }

    /// `HashAlignment#adjust`: a positive delta pads before `offset`, a negative one eats that
    /// many characters off the padding already there.
    pub(super) fn adjust(&mut self, offset: usize, delta: i64) {
        match delta.cmp(&0) {
            std::cmp::Ordering::Greater => {
                let width = usize::try_from(delta).unwrap_or(0);
                self.parts.push((offset, offset, " ".repeat(width)));
            }
            std::cmp::Ordering::Less => {
                let width = usize::try_from(-delta).unwrap_or(0);
                let mut start = offset;
                for _ in 0..width {
                    if start == 0 {
                        break;
                    }
                    start -= 1;
                    while start > 0 && !self.text.is_char_boundary(start) {
                        start -= 1;
                    }
                }
                self.parts.push((start, offset, String::new()));
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    /// The recorded corrections, in source order. Two that eat into the same padding would
    /// clobber each other upstream, which leaves the offense uncorrected rather than
    /// half-corrected.
    pub(super) fn finish(mut self) -> Vec<Edit> {
        self.parts
            .retain(|(start, end, replacement)| *start != *end || !replacement.is_empty());
        self.parts.sort_by_key(|(start, end, _)| (*start, *end));
        let mut cursor = 0;
        for (start, end, _) in &self.parts {
            if *start < cursor {
                return Vec::new();
            }
            cursor = *end;
        }
        self.parts
            .into_iter()
            .map(|(start, end, replacement)| Edit {
                start,
                end,
                replacement,
                safe: true,
            })
            .collect()
    }
}

/// One argument of a call, as `SendNode#arguments` hands it over: a single node, or the run of
/// `key: value` pairs and `**splat`s the parser folds into one brace-less `hash`.
pub(super) struct GroupedArgument<'tree> {
    pub(super) parts: Vec<Node<'tree>>,
    pub(super) range: Range<usize>,
    /// Whether the argument is the brace-less hash the parser synthesized.
    pub(super) hash_run: bool,
}

/// The arguments of a call, grouped the way upstream's parser presents them. An index read is a
/// call to `[]` there, so the nodes between its brackets are its arguments.
pub(super) fn grouped_arguments<'tree>(call: Node<'tree>) -> Vec<GroupedArgument<'tree>> {
    let mut cursor = call.walk();
    let children: Vec<Node<'tree>> = if call.kind() == "element_reference" {
        call.named_children(&mut cursor)
            .skip(1)
            .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
            .collect()
    } else {
        let Some(list) = call
            .children(&mut cursor)
            .find(|child| child.kind() == "argument_list")
        else {
            return Vec::new();
        };
        let mut inner = list.walk();
        list.named_children(&mut inner)
            .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
            .collect()
    };
    let mut arguments = Vec::new();
    let mut index = 0;
    while index < children.len() {
        if is_hash_element(children[index]) {
            let start = index;
            while index < children.len() && is_hash_element(children[index]) {
                index += 1;
            }
            let parts = children[start..index].to_vec();
            let range = parts[0].start_byte()..parts[parts.len() - 1].end_byte();
            arguments.push(GroupedArgument {
                parts,
                range,
                hash_run: true,
            });
        } else {
            arguments.push(GroupedArgument {
                parts: vec![children[index]],
                range: children[index].byte_range(),
                hash_run: false,
            });
            index += 1;
        }
    }
    arguments
}

/// `Alignment#display_column`: how far into its line a range starts, measured the way a terminal
/// would render it.
pub(super) fn display_column(context: &RuleContext<'_>, offset: usize) -> i64 {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    crate::display_width::display_width(&context.source.text()[start..offset])
}

/// `AlignmentCorrector.correct`: every line the node spans is moved sideways by `column_delta`.
pub(super) fn alignment_corrections(
    context: &RuleContext<'_>,
    expr: Range<usize>,
    column_delta: i64,
    taboo: &[Range<usize>],
) -> Vec<Edit> {
    if column_delta == 0 {
        return Vec::new();
    }
    let text = context.source.text();
    let mut edits = Vec::new();
    let mut line_begin = expr.start;
    for line in text[expr.clone()].split_inclusive('\n') {
        // The first position is the node's own start rather than its line's, which is what lets a
        // node that shares its line with something else be moved on its own.
        let range = if column_delta > 0 {
            line_begin..line_begin
        } else {
            let width = usize::try_from(-column_delta).unwrap_or(0);
            if text[line_begin..].starts_with(' ') {
                line_begin..(line_begin + width).min(text.len())
            } else {
                line_begin.saturating_sub(width)..line_begin
            }
        };
        if taboo
            .iter()
            .any(|range_| range.start >= range_.start && range.end <= range_.end)
        {
            line_begin += line.len();
            continue;
        }
        if column_delta > 0 {
            if !text[line_begin..].starts_with('\n') {
                let width = usize::try_from(column_delta).unwrap_or(0);
                edits.push(Edit {
                    start: line_begin,
                    end: line_begin,
                    replacement: " ".repeat(width),
                    safe: true,
                });
            }
        } else if !range.is_empty()
            && text[range.clone()]
                .bytes()
                .all(|byte| byte == b' ' || byte == b'\t')
        {
            edits.push(Edit {
                start: range.start,
                end: range.end,
                replacement: String::new(),
                safe: true,
            });
        }
        line_begin += line.len();
    }
    edits
}

/// The spans `AlignmentCorrector` refuses to move: the text inside a string literal, and the body
/// and terminator of a heredoc.
pub(super) fn string_interiors(
    context: &RuleContext<'_>,
    expr: &Range<usize>,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for node in context.nodes_of_any(&["string", "subshell"]) {
        if node.start_byte() < expr.start || node.end_byte() > expr.end {
            continue;
        }
        let count = node.child_count();
        if count < 2 {
            continue;
        }
        let (Some(first), Some(last)) = (
            node.child(0),
            node.child(u32::try_from(count).unwrap_or(0).saturating_sub(1)),
        ) else {
            continue;
        };
        if first.end_byte() <= last.start_byte() {
            ranges.push(first.end_byte()..last.start_byte());
        }
    }
    for node in context.nodes_of("heredoc_body") {
        if node.end_byte() > expr.start && node.start_byte() < expr.end {
            ranges.push(node.byte_range());
        }
    }
    ranges
}

/// Whether a `=begin` block comment lies inside the span, which stops the correction outright.
pub(super) fn holds_block_comment(context: &RuleContext<'_>, expr: &Range<usize>) -> bool {
    context.comment_ranges().iter().any(|comment| {
        comment.start >= expr.start
            && comment.end <= expr.end
            && context.source.text()[comment.clone()].starts_with("=begin")
    })
}

/// The literals of `kinds` written directly as arguments of `call`, paired with the call's opening
/// parenthesis.
///
/// This is `each_argument_node`, which walks each argument's subtree but stops at anything that is
/// a method call upstream -- so a literal nested inside another call belongs to that call instead.
pub(super) fn argument_literals<'tree>(
    context: &RuleContext<'_>,
    call: Node<'tree>,
    kinds: &[&str],
) -> Vec<(Node<'tree>, Node<'tree>)> {
    let mut cursor = call.walk();
    let Some(list) = call
        .children(&mut cursor)
        .find(|child| child.kind() == "argument_list")
    else {
        return Vec::new();
    };
    let Some(parenthesis) = list.child(0).filter(|child| child.kind() == "(") else {
        return Vec::new();
    };
    let parenthesis_line = context.source.line_column(parenthesis.start_byte()).0;

    let mut found = Vec::new();
    for argument in grouped_arguments(call) {
        for part in argument.parts {
            let mut stack = vec![part];
            while let Some(node) = stack.pop() {
                if kinds.contains(&node.kind()) {
                    if let Some(open) = literal_opening(node) {
                        if context.source.line_column(open.start_byte()).0 == parenthesis_line {
                            found.push((node, parenthesis));
                        }
                    }
                }
                if is_send_like(context, node) {
                    continue;
                }
                let mut inner = node.walk();
                stack.extend(node.named_children(&mut inner));
            }
        }
    }
    found
}

/// `loc.begin` of a literal: the brace, bracket or percent-literal opener it was written with.
pub(super) fn literal_opening<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let first = node.child(0)?;
    matches!(first.kind(), "{" | "[" | "%w(" | "%i(").then_some(first)
}

/// Whether upstream's parser calls the node a `send`, which is where `on_node`'s walk stops.
pub(super) fn is_send_like(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "call" | "element_reference" | "method_call" => true,
        "binary" => node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                !matches!(
                    &context.source.text()[operator.byte_range()],
                    "&&" | "||" | "and" | "or"
                )
            }),
        "unary" => node
            .child(0)
            .is_some_and(|operator| matches!(operator.kind(), "!" | "-" | "+" | "~" | "not")),
        _ => false,
    }
}

/// What the first element's indentation is measured against.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum IndentBase {
    LeftBraceOrBracket,
    FirstColumnAfterLeftParenthesis,
    ParentHashKey,
    StartOfLine,
}

/// `MultilineElementIndentation#indent_base`.
pub(super) fn indent_base(
    context: &RuleContext<'_>,
    open: Node<'_>,
    first: Option<Node<'_>>,
    parenthesis: Option<Node<'_>>,
    style: &str,
    brace_style: &str,
) -> (i64, IndentBase) {
    if style == brace_style {
        return (
            character_column(context, open.start_byte()),
            IndentBase::LeftBraceOrBracket,
        );
    }
    if let Some(pair) = parent_pair(open, first) {
        if key_and_value_begin_on_same_line(pair) && right_sibling_begins_later(pair) {
            return (
                character_column(context, pair.start_byte()),
                IndentBase::ParentHashKey,
            );
        }
    }
    if let Some(parenthesis) = parenthesis {
        if style == "special_inside_parentheses" {
            return (
                character_column(context, parenthesis.start_byte()) + 1,
                IndentBase::FirstColumnAfterLeftParenthesis,
            );
        }
    }
    (
        line_indentation(context, open.start_byte()),
        IndentBase::StartOfLine,
    )
}

/// `hash_pair_where_value_beginning_with`: the literal is the value of an enclosing pair.
fn parent_pair<'tree>(open: Node<'_>, first: Option<Node<'tree>>) -> Option<Node<'tree>> {
    let first = first?;
    let literal = first.parent()?;
    if literal_opening(literal) != Some(open) {
        return None;
    }
    literal.parent().filter(|parent| parent.kind() == "pair")
}

fn key_and_value_begin_on_same_line(pair: Node<'_>) -> bool {
    let (Some(key), Some(value)) = (
        pair.child_by_field_name("key"),
        pair.child_by_field_name("value"),
    ) else {
        return false;
    };
    key.start_position().row == value.start_position().row
}

fn right_sibling_begins_later(pair: Node<'_>) -> bool {
    let mut sibling = pair.next_named_sibling();
    while sibling.is_some_and(|node| matches!(node.kind(), "comment" | "heredoc_body")) {
        sibling = sibling.and_then(|node| node.next_named_sibling());
    }
    sibling.is_some_and(|sibling| pair.end_position().row < sibling.start_position().row)
}

/// A zero-based character column, which is the unit every `loc.column` is in.
pub(super) fn character_column(context: &RuleContext<'_>, offset: usize) -> i64 {
    context.source.line_column(offset).1 as i64 - 1
}

/// `source_line =~ /\S/`: where the line's first non-blank character sits.
pub(super) fn line_indentation(context: &RuleContext<'_>, offset: usize) -> i64 {
    let line = context.source.line_column(offset).0;
    let text = context.source.line(line);
    text.chars()
        .take_while(|character| character.is_whitespace() && *character != '\n')
        .count() as i64
}

/// Whether anything but blanks precedes `offset` on its line.
pub(super) fn preceded_by_code(context: &RuleContext<'_>, offset: usize) -> bool {
    !begins_its_line(context, offset)
}
