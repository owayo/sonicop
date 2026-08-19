//! `Layout/SpaceInsideArrayLiteralBrackets`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const NO_SPACE_COMMAND: &str = "Do not use";
const SPACE_COMMAND: &str = "Use";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "no_space".to_owned());
    let empty_style: String = context
        .setting("EnforcedStyleForEmptyBrackets")
        .unwrap_or_else(|| "no_space".to_owned());
    let mut reporter = Reporter {
        context,
        style: &style,
        empty_style: &empty_style,
        reported: HashSet::new(),
        corrected: HashSet::new(),
    };
    // `on_array` and `on_array_pattern`. A pattern spelled `Foo[1, 2]` is a `const_pattern`
    // upstream, and `find_node_with_brackets` climbs to it so that the constant is part of the
    // token run; the grammar here already puts the constant inside the pattern node.
    for node in context.nodes_of_any(&["array", "array_pattern"]) {
        reporter.inspect(node, offenses);
    }
}

struct Reporter<'a, 'b> {
    context: &'a RuleContext<'b>,
    style: &'a str,
    empty_style: &'a str,
    /// Ranges already reported. RuboCop keeps this set per cop and per file, so a second offense
    /// covering the very same span is dropped before it can reach the corrector.
    reported: HashSet<(usize, usize)>,
    /// Nodes whose correction has been emitted. Upstream corrects a node once, from whichever
    /// offense it reports first, and every later offense on that node carries no corrector at all
    /// -- which is what leaves it `correctable: false`.
    corrected: HashSet<usize>,
}

impl Reporter<'_, '_> {
    fn inspect(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        // `square_brackets?`: a literal written `%w[...]` or an implicit array such as `a = 1, 2`
        // carries no bracket token and is left alone.
        let (Some(left), Some(right)) = brackets(node) else {
            return;
        };
        let text = self.context.source.text();
        let inner = left.end_byte()..right.start_byte();
        let between = &text[inner.clone()];

        // `empty_brackets?` asks whether the two bracket tokens are adjacent in the token stream.
        // Nothing but whitespace between them means no token came in between: a percent-free array
        // literal never holds a `tNL`, and a comment would show up as non-whitespace text.
        if between.trim().is_empty() {
            self.empty_offenses(node, &left, &right, offenses);
            return;
        }

        let single_line = node.start_position().row == node.end_position().row;
        let end_ok = !single_line && end_has_own_line(text, right.start_byte());
        match self.style {
            "space" => {
                let start_ok = next_to_newline(text, left.end_byte());
                self.space_offenses(node, &left, Some(&right), start_ok, end_ok, offenses);
            }
            "compact" => {
                let start_ok = next_to_newline(text, left.end_byte());
                self.compact_offenses(node, &left, &right, start_ok, end_ok, offenses);
            }
            _ => {
                // For `no_space` the opening bracket is excused only by a comment token following
                // it, not by a line break: trailing blanks after `[` are still an offense.
                let start_ok = next_to_comment(self.context, text, left.end_byte());
                self.no_space_offenses(node, Some(&left), Some(&right), start_ok, end_ok, offenses);
            }
        }
    }

    fn empty_offenses(
        &mut self,
        node: Node<'_>,
        left: &Node<'_>,
        right: &Node<'_>,
        offenses: &mut Vec<Offense>,
    ) {
        let range = left.start_byte()..right.end_byte();
        let inner = left.end_byte()..right.start_byte();
        let text = self.context.source.text();
        let command = if self.empty_style == "space" {
            if space_between(text, left, right) {
                return;
            }
            "Use one"
        } else {
            if inner.is_empty() {
                return;
            }
            NO_SPACE_COMMAND
        };
        let replacement = if self.empty_style == "space" {
            " ".to_owned()
        } else {
            String::new()
        };
        if !self.reported.insert((range.start, range.end)) {
            return;
        }
        // The empty case corrects from every offense it reports rather than once per node, but
        // only one of the two conditions can hold at a time so it never reports twice.
        self.corrected.insert(node.id());
        offenses.push(
            self.context
                .offense(
                    format!("{command} space inside empty array brackets."),
                    range,
                )
                .corrected_by(Edit {
                    start: inner.start,
                    end: inner.end,
                    replacement,
                    safe: true,
                }),
        );
    }

    fn no_space_offenses(
        &mut self,
        node: Node<'_>,
        left: Option<&Node<'_>>,
        right: Option<&Node<'_>>,
        start_ok: bool,
        end_ok: bool,
        offenses: &mut Vec<Offense>,
    ) {
        let text = self.context.source.text();
        if !start_ok && left.is_some_and(|token| extra_space_after(text, token)) {
            let range = space_after(text, left.unwrap().end_byte());
            self.report(node, range, NO_SPACE_COMMAND, offenses);
        }
        if end_ok || !right.is_some_and(|token| extra_space_before(text, token)) {
            return;
        }
        let range = space_before(text, right.unwrap().start_byte());
        self.report(node, range, NO_SPACE_COMMAND, offenses);
    }

    fn space_offenses(
        &mut self,
        node: Node<'_>,
        left: &Node<'_>,
        right: Option<&Node<'_>>,
        start_ok: bool,
        end_ok: bool,
        offenses: &mut Vec<Offense>,
    ) {
        let text = self.context.source.text();
        // `space_offense(node, token, :none, ...)`: the offense sits on the bracket itself, not in
        // the gap the correction fills. Reporting the empty range after `[` would name the column
        // one past the bracket and give the offense no length.
        if !start_ok && !extra_space_after(text, left) {
            self.report(node, left.byte_range(), SPACE_COMMAND, offenses);
        }
        let Some(right) = right else { return };
        if end_ok || extra_space_before(text, right) {
            return;
        }
        self.report(node, right.byte_range(), SPACE_COMMAND, offenses);
    }

    /// `compact_offenses`: successive brackets are pushed together, and every other bracket wants
    /// the space the `space` style asks for.
    fn compact_offenses(
        &mut self,
        node: Node<'_>,
        left: &Node<'_>,
        right: &Node<'_>,
        start_ok: bool,
        end_ok: bool,
        offenses: &mut Vec<Offense>,
    ) {
        let text = self.context.source.text();
        let nested_left = next_is_left_bracket(text, left.end_byte());
        if nested_left && extra_space_after(text, left) {
            let range = space_after(text, left.end_byte());
            self.report(node, range, NO_SPACE_COMMAND, offenses);
        } else if !nested_left {
            self.space_offenses(node, left, None, start_ok, true, offenses);
        }

        let nested_right = previous_is_right_bracket(text, right.start_byte());
        if nested_right && extra_space_before(text, right) {
            let range = space_before(text, right.start_byte());
            self.report(node, range, NO_SPACE_COMMAND, offenses);
        } else if !nested_right {
            let offset = right.start_byte();
            if !end_ok && !extra_space_before(text, right) {
                self.report(node, offset..offset, SPACE_COMMAND, offenses);
            }
        }
    }

    fn report(
        &mut self,
        node: Node<'_>,
        range: Range<usize>,
        command: &str,
        offenses: &mut Vec<Offense>,
    ) {
        if !self.reported.insert((range.start, range.end)) {
            return;
        }
        let mut offense = self
            .context
            .offense(format!("{command} space inside array brackets."), range);
        if self.corrected.insert(node.id()) {
            offense = offense.corrected_by_all(self.corrections(node));
        }
        offenses.push(offense);
    }

    /// The whole of the node's correction, which upstream applies in one go from the first offense
    /// it reports. Both bracket sides are rewritten together, so a side that was excused from
    /// reporting still gets corrected.
    fn corrections(&self, node: Node<'_>) -> Vec<Edit> {
        let (Some(left), Some(right)) = brackets(node) else {
            return Vec::new();
        };
        let text = self.context.source.text();
        let mut edits = Vec::new();
        match self.style {
            "space" => {
                // `add_space` looks at any whitespace, so a bracket followed by a line break is
                // already spaced out.
                if !has_space_after(text, left.end_byte()) {
                    edits.push(insert(left.end_byte()));
                }
                if !has_space_before(text, right.start_byte()) {
                    edits.push(insert(right.start_byte()));
                }
            }
            "compact" => {
                if next_is_left_bracket(text, left.end_byte()) {
                    let range = whitespace_after(text, left.end_byte());
                    if !range.is_empty() {
                        edits.push(remove(range));
                    }
                } else if !has_space_after(text, left.end_byte()) {
                    edits.push(insert(left.end_byte()));
                }
                if previous_is_right_bracket(text, right.start_byte()) {
                    let range = whitespace_before(text, right.start_byte());
                    if !range.is_empty() {
                        edits.push(remove(range));
                    }
                } else if !has_space_before(text, right.start_byte()) {
                    edits.push(insert(right.start_byte()));
                }
            }
            _ => {
                // `remove_space` clears the run of spaces and tabs beside each bracket, which is
                // empty when only a line break separates them.
                let head = space_after(text, left.end_byte());
                if has_space_after(text, left.end_byte()) && !head.is_empty() {
                    edits.push(remove(head));
                }
                let tail = space_before(text, right.start_byte());
                if has_space_before(text, right.start_byte()) && !tail.is_empty() {
                    edits.push(remove(tail));
                }
            }
        }
        edits
    }
}

fn insert(offset: usize) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement: " ".to_owned(),
        safe: true,
    }
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// Whitespace runs including line breaks, which `compact` collapses.
fn whitespace_after(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = offset;
    while matches!(bytes.get(end), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        end += 1;
    }
    offset..end
}

fn whitespace_before(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t' | b'\r' | b'\n') {
        start -= 1;
    }
    start..offset
}

/// The node's own `[` and `]`. Nested brackets belong to child nodes, so scanning direct children
/// yields exactly the pair `tokens.find(&:left_bracket?)` and `tokens.reverse_each.find(...)` pick.
fn brackets<'tree>(node: Node<'tree>) -> (Option<Node<'tree>>, Option<Node<'tree>>) {
    let mut cursor = node.walk();
    let mut left = None;
    let mut right = None;
    for child in node.children(&mut cursor) {
        match child.kind_str() {
            "[" if left.is_none() => left = Some(child),
            "]" => right = Some(child),
            _ => {}
        }
    }
    (left, right)
}

/// `token.space_after?` restricted to `[ \t]`, which is what `extra_space?` tests.
fn extra_space_after(text: &str, token: &Node<'_>) -> bool {
    matches!(text.as_bytes().get(token.end_byte()), Some(b' ' | b'\t'))
}

fn extra_space_before(text: &str, token: &Node<'_>) -> bool {
    token.start_byte() > 0
        && matches!(
            text.as_bytes().get(token.start_byte() - 1),
            Some(b' ' | b'\t')
        )
}

/// `token.space_after?` itself, which any whitespace satisfies.
fn has_space_after(text: &str, offset: usize) -> bool {
    text[offset..].starts_with(|character: char| character.is_whitespace())
}

fn has_space_before(text: &str, offset: usize) -> bool {
    let probe = if offset == 0 { 0 } else { offset - 1 };
    text[probe..].starts_with(|character: char| character.is_whitespace())
}

fn space_after(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = offset;
    while matches!(bytes.get(end), Some(b' ' | b'\t')) {
        end += 1;
    }
    offset..end
}

fn space_before(text: &str, offset: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}

/// `next_to_newline?`: the token after the opening bracket sits on another line.
fn next_to_newline(text: &str, offset: usize) -> bool {
    text[offset..]
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .any(|byte| byte == b'\n')
}

/// `next_to_comment?`: the token after the opening bracket is a comment. Line breaks are skipped
/// with the rest of the whitespace, because the token stream holds no newline token inside an
/// array literal.
fn next_to_comment(context: &RuleContext<'_>, text: &str, offset: usize) -> bool {
    let next = offset + count_whitespace(text, offset);
    text.as_bytes().get(next) == Some(&b'#')
        && context
            .comment_ranges()
            .binary_search_by(|range| range.start.cmp(&next))
            .is_ok()
}

fn count_whitespace(text: &str, offset: usize) -> usize {
    text[offset..]
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .count()
}

/// `multi_dimensional_array?` on the opening side: the next token is another opening bracket.
fn next_is_left_bracket(text: &str, offset: usize) -> bool {
    let next = offset + count_whitespace(text, offset);
    text.as_bytes().get(next) == Some(&b'[')
}

fn previous_is_right_bracket(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t' | b'\r' | b'\n') {
        start -= 1;
    }
    start > 0 && bytes[start - 1] == b']'
}

/// `end_has_own_line?`: nothing but whitespace precedes the closing bracket on its line.
fn end_has_own_line(text: &str, offset: usize) -> bool {
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    !text[line_start..offset].contains(|character: char| !character.is_whitespace())
}

/// `space_between?`: exactly one space separates the two brackets.
fn space_between(text: &str, left: &Node<'_>, right: &Node<'_>) -> bool {
    left.end_byte() + 1 == right.start_byte() && text.as_bytes().get(left.end_byte()) == Some(&b' ')
}
