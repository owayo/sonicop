//! `PrecedingFollowingAlignment`: whether a token lines up with something on a neighbouring line.
//!
//! RuboCop's mixin lets a cop excuse padding that was written to align code with the line above or
//! below it -- `Layout/SpaceAroundOperators`, `Layout/ExtraSpacing` and
//! `Layout/SpaceBeforeFirstArg` all ask it before reporting a run of spaces. The three cops
//! disagree about almost everything else, so the shared state (the file's lines, the comment lines
//! that do not count as alignment targets, and the `=`-ish operators that do) is gathered once
//! here.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::Node;

use super::support::comments;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::source::SourceFile;

/// The comparisons of RuboCop's `ASSIGNMENT_OR_COMPARISON_TOKENS`, spelled as source rather than
/// as lexer token names. `<<` and the assignment operators are recognised structurally instead.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">="];

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Aligned {
    Yes,
    No,
    None,
}

/// What kind of lexer token an operator would have been. `aligned_equals_operator?` treats an
/// append and an assignment as interchangeable, so the two have to stay apart from the
/// comparisons that only ever match themselves.
#[derive(Clone, Copy, PartialEq)]
enum TokenKind {
    EqualSign,
    Lshift,
    Comparison,
}

#[derive(Clone, Copy)]
struct EqualsToken {
    start: usize,
    /// The 0-based character column just past the operator.
    last_column: usize,
    kind: TokenKind,
}

/// Everything `PrecedingFollowingAlignment` reads out of a file: the lines, the comment lines it
/// will not align against, and the `=`-ish operators it aligns with.
pub(super) struct Alignment<'src> {
    source: &'src SourceFile,
    /// The file's lines without their line breaks, as `processed_source.lines` holds them.
    lines: Vec<&'src str>,
    indents: Vec<usize>,
    blank: Vec<bool>,
    /// 1-based lines carrying a comment that starts the line.
    comment_lines: HashSet<usize>,
    /// 1-based line to its first assignment or comparison operator.
    equals_tokens: HashMap<usize, EqualsToken>,
    /// `assignment_tokens`: 1-based line to the first assignment `=` written on it, ignoring the
    /// parameter defaults and endless `def`s RuboCop's `remove_equals_in_def` drops. Only the
    /// first one per line is kept, which is what `uniq(&:line)` leaves.
    assignment_tokens: HashMap<usize, Range<usize>>,
}

impl<'src> Alignment<'src> {
    pub(super) fn new(context: &RuleContext<'src>) -> Self {
        let source: &'src SourceFile = context.source;
        // `Parser::Source::Buffer#source_lines` keeps a trailing empty line for a file ending in
        // a newline, which is exactly what `SourceFile::line_count` counts.
        let lines: Vec<&str> = (1..=source.line_count())
            .map(|line| {
                let text = source.line(line);
                text.strip_suffix('\n').unwrap_or(text)
            })
            .collect();
        let indents = lines
            .iter()
            .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
            .collect();
        let blank = lines
            .iter()
            .map(|line| line.chars().all(char::is_whitespace))
            .collect();

        // `processed_source.comments`, which the `#` the grammar finds in a heredoc body is not
        // one of: that text never reached the lexer as a comment, so it stays a line the mixin
        // will align against.
        let mut comment_lines = HashSet::new();
        for comment in comments(context) {
            let (line, column) = source.line_column(comment.start);
            if lines[line - 1]
                .chars()
                .position(|character| !character.is_whitespace())
                .is_some_and(|index| index + 1 == column)
            {
                comment_lines.insert(line);
            }
        }

        let mut alignment = Self {
            source,
            lines,
            indents,
            blank,
            comment_lines,
            equals_tokens: HashMap::new(),
            assignment_tokens: HashMap::new(),
        };
        alignment.collect_equals_tokens(context);
        alignment
    }

    fn collect_equals_tokens(&mut self, context: &RuleContext<'_>) {
        for node in context.nodes() {
            let (operator, kind, assigns) = match node.kind_str() {
                "assignment" => (
                    node.field("left").and_then(|left| {
                        node.field("right")
                            .and_then(|right| operator_between(node, left, right))
                    }),
                    TokenKind::EqualSign,
                    true,
                ),
                "operator_assignment" => (node.field("operator"), TokenKind::EqualSign, true),
                // An optional parameter's `=` and an endless `def`'s are still tokens to align
                // against, even though `assignment_lines` leaves them out.
                "optional_parameter" | "method" => {
                    (child_of_kind(node, "="), TokenKind::EqualSign, false)
                }
                // `remove_equals_in_def` walks `each_node(:optarg, :def)`, which never reaches a
                // `defs`, so the `=` of an endless singleton method stays an assignment token.
                "singleton_method" => (child_of_kind(node, "="), TokenKind::EqualSign, true),
                "singleton_class" => (child_of_kind(node, "<<"), TokenKind::Lshift, false),
                "binary" => {
                    let operator = node.field("operator");
                    let text = operator.map(|operator| context.source.node_text(operator));
                    match text {
                        Some("<<") => (operator, TokenKind::Lshift, false),
                        Some(text) if COMPARISON_OPERATORS.contains(&text) => {
                            (operator, TokenKind::Comparison, false)
                        }
                        _ => (None, TokenKind::Comparison, false),
                    }
                }
                _ => (None, TokenKind::Comparison, false),
            };
            let Some(operator) = operator else {
                continue;
            };
            let (line, _) = self.source.line_column(operator.start_byte());
            let (_, end_column) = self.source.line_column(operator.end_byte());
            let token = EqualsToken {
                start: operator.start_byte(),
                last_column: end_column - 1,
                kind,
            };
            self.equals_tokens
                .entry(line)
                .and_modify(|current| {
                    if token.start < current.start {
                        *current = token;
                    }
                })
                .or_insert(token);
            if assigns {
                self.assignment_tokens
                    .entry(line)
                    .and_modify(|current| {
                        if operator.start_byte() < current.start {
                            *current = operator.byte_range();
                        }
                    })
                    .or_insert_with(|| operator.byte_range());
            }
        }
    }

    /// `processed_source.lines.size`.
    pub(super) fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// `processed_source.lines[line - 1]`.
    pub(super) fn line(&self, line: usize) -> &'src str {
        self.lines.get(line - 1).copied().unwrap_or("")
    }

    /// The first `=` of `line`, when the line carries an assignment `RuboCop` would align.
    pub(super) fn assignment_token(&self, line: usize) -> Option<&Range<usize>> {
        self.assignment_tokens.get(&line)
    }

    /// `all_relevant_assignment_lines`: the assignment lines of the block `line` sits in, searched
    /// upwards and downwards from it.
    pub(super) fn all_relevant_assignment_lines(&self, line: usize) -> Vec<usize> {
        let preceding: Vec<usize> = (1..=line).rev().collect();
        let following: Vec<usize> = (line..=self.line_count()).collect();
        let mut lines = self.relevant_assignment_lines(&preceding);
        lines.extend(self.relevant_assignment_lines(&following));
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    fn slice(&self, range: &Range<usize>) -> &str {
        &self.source.text()[range.clone()]
    }

    pub(super) fn aligned_with_something(&self, range: &Range<usize>) -> bool {
        self.aligned_with_adjacent_line(range, Predicate::Token)
    }

    pub(super) fn aligned_with_operator(&self, range: &Range<usize>) -> bool {
        self.aligned_with_adjacent_line(range, Predicate::Operator)
    }

    fn aligned_with_adjacent_line(&self, range: &Range<usize>, predicate: Predicate) -> bool {
        let (line, _) = self.source.line_column(range.start);
        // RuboCop searches the preceding lines first, then the following ones; both lists hold
        // 0-based indices into `lines`.
        let preceding: Vec<usize> = (0..line.saturating_sub(1)).rev().collect();
        let following: Vec<usize> = (line..self.line_count()).collect();
        let candidates = [preceding, following];
        if self.aligned_with_any_line(&candidates, range, None, predicate) {
            return true;
        }
        // Failing that, the nearest line indented like this one gets to answer instead.
        let base = self.lines[line - 1]
            .chars()
            .position(|character| !character.is_whitespace());
        base.is_some_and(|indent| {
            self.aligned_with_any_line(&candidates, range, Some(indent), predicate)
        })
    }

    fn aligned_with_any_line(
        &self,
        candidates: &[Vec<usize>; 2],
        range: &Range<usize>,
        indent: Option<usize>,
        predicate: Predicate,
    ) -> bool {
        candidates
            .iter()
            .any(|lines| self.aligned_with_line(lines, range, indent, predicate))
    }

    /// The first line of `lines` that is neither blank nor a comment (and, when `indent` is
    /// given, is indented the same) settles the question on its own.
    fn aligned_with_line(
        &self,
        lines: &[usize],
        range: &Range<usize>,
        indent: Option<usize>,
        predicate: Predicate,
    ) -> bool {
        for &index in lines {
            if self.comment_lines.contains(&(index + 1)) {
                continue;
            }
            let line = self.lines[index];
            let Some(first) = line
                .chars()
                .position(|character| !character.is_whitespace())
            else {
                continue;
            };
            if indent.is_some_and(|indent| indent != first) {
                continue;
            }
            let matched = match predicate {
                Predicate::Token => self.aligned_words(range, line),
                Predicate::Operator => self.aligned_identical(range, line),
            };
            return matched || self.aligned_equals_operator(range, index + 1);
        }
        false
    }

    fn aligned_words(&self, range: &Range<usize>, line: &str) -> bool {
        let (_, column) = self.source.line_column(range.start);
        let left_edge = column - 1;
        let characters: Vec<char> = line.chars().collect();
        // `line[left_edge - 1, 2]` in Ruby, where a zero edge reads the line's last character
        // and so can never hold the two-character match.
        if left_edge > 0
            && characters
                .get(left_edge - 1..left_edge + 1)
                .is_some_and(|pair| pair[0].is_whitespace() && !pair[1].is_whitespace())
        {
            return true;
        }
        self.same_text_at(range, &characters, left_edge)
    }

    fn aligned_identical(&self, range: &Range<usize>, line: &str) -> bool {
        let (_, column) = self.source.line_column(range.start);
        let characters: Vec<char> = line.chars().collect();
        self.same_text_at(range, &characters, column - 1)
    }

    fn same_text_at(&self, range: &Range<usize>, characters: &[char], column: usize) -> bool {
        let token = self.slice(range);
        let width = token.chars().count();
        characters
            .get(column..column + width)
            .is_some_and(|slice| slice.iter().copied().eq(token.chars()))
    }

    /// Whether the operator ends in the same column as the first assignment or comparison
    /// operator of `line`, which is how RuboCop lets an `=` line up with the one above it.
    fn aligned_equals_operator(&self, range: &Range<usize>, line: usize) -> bool {
        let Some(token) = self.equals_tokens.get(&line) else {
            return false;
        };
        let source = self.slice(range);
        let (_, end_column) = self.source.line_column(range.end);
        if end_column - 1 != token.last_column {
            return false;
        }
        source.ends_with('=')
            || (source == "<<" && token.kind == TokenKind::EqualSign)
            || (source.ends_with('=') && token.kind == TokenKind::Lshift)
    }

    pub(super) fn aligned_with_preceding_equals(&self, range: &Range<usize>) -> Aligned {
        let (line, _) = self.source.line_column(range.start);
        let lines: Vec<usize> = (1..=line).rev().collect();
        self.aligned_with_equals_sign(range, &lines)
    }

    pub(super) fn aligned_with_subsequent_equals(&self, range: &Range<usize>) -> Aligned {
        let (line, _) = self.source.line_column(range.start);
        let lines: Vec<usize> = (line..=self.line_count()).collect();
        self.aligned_with_equals_sign(range, &lines)
    }

    fn aligned_with_equals_sign(&self, range: &Range<usize>, lines: &[usize]) -> Aligned {
        let (line, _) = self.source.line_column(range.start);
        let token_indent = self.indentation(line);
        let assignments = self.relevant_assignment_lines(lines);
        // The operator's own line comes first; the next assignment of the same block decides.
        let Some(&relevant) = assignments.get(1) else {
            return Aligned::None;
        };
        if self.indentation(relevant) < token_indent {
            return Aligned::None;
        }
        if self.aligned_equals_operator(range, relevant) {
            Aligned::Yes
        } else {
            Aligned::No
        }
    }

    /// The lines of the same block, at the same indentation, that hold an assignment. The walk
    /// stops at the first line leaving the block, or at the blank line ending it.
    fn relevant_assignment_lines(&self, lines: &[usize]) -> Vec<usize> {
        let mut result = Vec::new();
        let Some(&first) = lines.first() else {
            return result;
        };
        let original_indent = self.indentation(first);
        let mut indent_at_level = true;
        for &line in lines {
            let current_indent = self.indentation(line);
            let blank = self.blank.get(line - 1).copied().unwrap_or(true);
            if (current_indent < original_indent && !blank) || (indent_at_level && blank) {
                break;
            }
            if self.assignment_tokens.contains_key(&line) && current_indent == original_indent {
                result.push(line);
            }
            if !blank {
                indent_at_level = current_indent == original_indent;
            }
        }
        result
    }

    fn indentation(&self, line: usize) -> usize {
        self.indents.get(line - 1).copied().unwrap_or(0)
    }
}

#[derive(Clone, Copy)]
enum Predicate {
    Token,
    Operator,
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == kind)
}

fn operator_between<'tree>(
    node: Node<'tree>,
    left: Node<'_>,
    right: Node<'_>,
) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).find(|child| {
        child.start_byte() >= left.end_byte() && child.end_byte() <= right.start_byte()
    })
}
