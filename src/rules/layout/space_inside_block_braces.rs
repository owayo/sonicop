//! `Layout/SpaceInsideBlockBraces`.

use tree_sitter::Node;

use super::support::{character_column, parser_node_start};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::is_ruby_space;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Style {
    Space,
    NoSpace,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        style: match context.setting::<String>("EnforcedStyle").as_deref() {
            Some("no_space") => Style::NoSpace,
            _ => Style::Space,
        },
        empty_style: match context
            .setting::<String>("EnforcedStyleForEmptyBraces")
            .as_deref()
        {
            Some("space") => Style::Space,
            _ => Style::NoSpace,
        },
        space_before_parameters: context
            .setting::<bool>("SpaceBeforeBlockParameters")
            .unwrap_or(true),
    };
    // A `do ... end` block is a node of its own kind here, so `keywords?` needs no test.
    for node in context.nodes_of("block") {
        cop.on_block(node, offenses);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    style: Style,
    empty_style: Style,
    space_before_parameters: bool,
}

impl Cop<'_, '_> {
    fn text(&self) -> &str {
        self.context.source.text()
    }

    fn on_block(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(left) = node.child(0).filter(|child| child.kind_str() == "{") else {
            return;
        };
        let Some(right) = last_child(node).filter(|child| child.kind_str() == "}") else {
            return;
        };
        // The `block` node upstream spans the call it hangs off, so it begins at the receiver.
        let expr_start = parser_node_start(node);
        // `BlockNode#single_line?` is overridden upstream to compare `loc.begin` with `loc.end`,
        // which is the braces rather than the whole expression -- so a one-line block hanging off
        // a receiver that took several lines is still a single-line block.
        let single_line = self.context.source.line_column(left.start_byte()).0
            == self.context.source.line_column(right.start_byte()).0;
        // Empty braces spread over two lines are left alone: correcting them to a single line
        // would fight the correction this same cop makes to single-line empty braces.
        if !holds_a_statement(node) && !single_line {
            return;
        }
        let braces = Braces {
            left,
            right,
            single_line,
            expr_start,
        };
        self.check_inside(node, &braces, offenses);
    }

    fn check_inside(&self, node: Node<'_>, braces: &Braces<'_>, offenses: &mut Vec<Offense>) {
        let (left, right) = (braces.left, braces.right);
        if left.end_byte() == right.start_byte() {
            if self.empty_style == Style::Space {
                self.offense(
                    left.start_byte(),
                    right.end_byte(),
                    "Space missing inside empty braces.",
                    offenses,
                );
            }
            return;
        }
        let inner = &self.text()[left.end_byte()..right.start_byte()];
        if inner.bytes().any(|byte| !is_ruby_space(byte)) {
            self.braces_with_contents_inside(node, braces, inner, offenses);
        } else if self.empty_style == Style::NoSpace {
            self.offense(
                left.end_byte(),
                right.start_byte(),
                "Space inside empty braces detected.",
                offenses,
            );
        }
    }

    fn braces_with_contents_inside(
        &self,
        node: Node<'_>,
        braces: &Braces<'_>,
        inner: &str,
        offenses: &mut Vec<Offense>,
    ) {
        // `node.arguments.loc.begin`, which is the `|` of `{ |x| ... }`. A lambda literal keeps its
        // parameters outside the braces and opens them with `(`, which is not a pipe either way.
        let pipe = node
            .field("parameters")
            .and_then(|parameters| parameters.child(0))
            .filter(|child| child.kind_str() == "|");
        self.check_left_brace(inner, braces.left, pipe, offenses);
        self.check_right_brace(inner, braces, offenses);
    }

    fn check_left_brace(
        &self,
        inner: &str,
        left: Node<'_>,
        pipe: Option<Node<'_>>,
        offenses: &mut Vec<Offense>,
    ) {
        if inner.bytes().next().is_some_and(|byte| !is_ruby_space(byte)) {
            self.no_space_inside_left_brace(left, pipe, offenses);
        } else {
            self.space_inside_left_brace(left, pipe, offenses);
        }
    }

    fn no_space_inside_left_brace(
        &self,
        left: Node<'_>,
        pipe: Option<Node<'_>>,
        offenses: &mut Vec<Offense>,
    ) {
        match pipe {
            Some(pipe) => {
                if left.end_byte() == pipe.start_byte() && self.space_before_parameters {
                    self.offense(
                        left.start_byte(),
                        pipe.end_byte(),
                        "Space between { and | missing.",
                        offenses,
                    );
                }
            }
            // The position after the left brace, which is what tells space missing to its left
            // from space missing to its right apart once the correction runs.
            None => self.no_space(
                left.end_byte(),
                offset_by_chars(self.text(), left.end_byte(), 1),
                "Space missing inside {.",
                offenses,
            ),
        }
    }

    fn space_inside_left_brace(
        &self,
        left: Node<'_>,
        pipe: Option<Node<'_>>,
        offenses: &mut Vec<Offense>,
    ) {
        match pipe {
            Some(pipe) => {
                if !self.space_before_parameters {
                    self.offense(
                        left.end_byte(),
                        pipe.start_byte(),
                        "Space between { and | detected.",
                        offenses,
                    );
                }
            }
            None => {
                let end = expand_space(self.text(), left.end_byte(), Direction::Right);
                self.space(left.end_byte(), end, "Space inside { detected.", offenses);
            }
        }
    }

    fn check_right_brace(&self, inner: &str, braces: &Braces<'_>, offenses: &mut Vec<Offense>) {
        let (left, right) = (braces.left, braces.right);
        if braces.single_line
            && inner
                .bytes()
                .next_back()
                .is_some_and(|byte| !is_ruby_space(byte))
        {
            self.no_space(
                right.start_byte(),
                right.end_byte(),
                "Space missing inside }.",
                offenses,
            );
            return;
        }
        let column = character_column(self.context, braces.expr_start);
        let multiline_braces = self.context.source.line_column(left.start_byte()).0
            != self.context.source.line_column(right.start_byte()).0;
        let right_column = character_column(self.context, right.start_byte());
        if multiline_braces && (column == right_column || column == last_line_spaces(inner)) {
            return;
        }
        self.space_inside_right_brace(inner, right, column, right_column, offenses);
    }

    fn space_inside_right_brace(
        &self,
        inner: &str,
        right: Node<'_>,
        column: i64,
        right_column: i64,
        offenses: &mut Vec<Offense>,
    ) {
        let text = self.text();
        let space_start = expand_space(text, right.start_byte(), Direction::Left);
        let mut begin_pos = space_start;
        let mut end_pos = right.start_byte();
        if text[space_start..end_pos].contains('\n') {
            begin_pos = offset_by_chars(text, end_pos, column - right_column);
        }
        if inner.ends_with(']') {
            end_pos = offset_by_chars(text, end_pos, -1);
            begin_pos = offset_by_chars(text, end_pos, column - last_line_spaces(inner));
        }
        self.space(begin_pos, end_pos, "Space inside } detected.", offenses);
    }

    fn no_space(
        &self,
        begin_pos: usize,
        end_pos: usize,
        message: &str,
        offenses: &mut Vec<Offense>,
    ) {
        if self.style == Style::Space {
            self.offense(begin_pos, end_pos, message, offenses);
        }
    }

    fn space(&self, begin_pos: usize, end_pos: usize, message: &str, offenses: &mut Vec<Offense>) {
        if self.style == Style::NoSpace {
            self.offense(begin_pos, end_pos, message, offenses);
        }
    }

    fn offense(
        &self,
        begin_pos: usize,
        end_pos: usize,
        message: &str,
        offenses: &mut Vec<Offense>,
    ) {
        if begin_pos > end_pos {
            return;
        }
        let source = &self.text()[begin_pos..end_pos];
        let edit = if source.bytes().any(is_ruby_space) {
            Edit {
                start: begin_pos,
                end: end_pos,
                replacement: String::new(),
                safe: true,
            }
        } else if source == "{}" || source == "{|" {
            Edit {
                start: begin_pos,
                end: end_pos,
                replacement: format!("{{ {}", &source[1..]),
                safe: true,
            }
        } else {
            Edit {
                start: begin_pos,
                end: begin_pos,
                replacement: " ".to_owned(),
                safe: true,
            }
        };
        offenses.push(
            self.context
                .offense(message, begin_pos..end_pos)
                .corrected_by(edit),
        );
    }
}

/// The braces of one block, and where the expression they close begins.
struct Braces<'tree> {
    left: Node<'tree>,
    right: Node<'tree>,
    single_line: bool,
    expr_start: usize,
}

fn last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)
}

/// Whether `BlockNode#body` would be anything but `nil` upstream. A `;` only separates statements
/// there and a comment is not part of the tree at all, so braces holding nothing else are empty
/// even though the grammar here parks an `empty_statement` or a `comment` between them.
fn holds_a_statement(node: Node<'_>) -> bool {
    let Some(body) = node.field("body") else {
        return false;
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .any(|child| !matches!(child.kind_str(), "empty_statement" | "comment"))
}

/// `inner.split("\n").last.count(' ')`: every space on the last line the braces hold, not only the
/// ones that indent it.
fn last_line_spaces(inner: &str) -> i64 {
    let mut lines: Vec<&str> = inner.split('\n').collect();
    // `String#split` drops the trailing empty fields.
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.last().map_or(0, |line| {
        line.bytes().filter(|byte| *byte == b' ').count() as i64
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Direction {
    Left,
    Right,
}

/// `RangeHelp#range_with_surrounding_space` with the defaults this cop passes: the run of spaces
/// and tabs, and then the run of newlines beyond it.
fn expand_space(text: &str, offset: usize, direction: Direction) -> usize {
    let bytes = text.as_bytes();
    let mut position = offset;
    let peek = |position: usize| match direction {
        Direction::Left => (position > 0).then(|| bytes[position - 1]),
        Direction::Right => bytes.get(position).copied(),
    };
    let step = |position: usize| match direction {
        Direction::Left => position - 1,
        Direction::Right => position + 1,
    };
    while peek(position).is_some_and(|byte| byte == b' ' || byte == b'\t') {
        position = step(position);
    }
    while peek(position).is_some_and(|byte| byte == b'\n') {
        position = step(position);
    }
    position
}

/// Moves `offset` by `delta` characters. Upstream addresses source by character, so a span it
/// builds by adding a column difference to a position is that many characters wide.
fn offset_by_chars(text: &str, offset: usize, delta: i64) -> usize {
    let mut position = offset.min(text.len());
    match delta.cmp(&0) {
        std::cmp::Ordering::Greater => {
            for _ in 0..delta {
                if position >= text.len() {
                    break;
                }
                position += 1;
                while position < text.len() && !text.is_char_boundary(position) {
                    position += 1;
                }
            }
        }
        std::cmp::Ordering::Less => {
            for _ in 0..-delta {
                if position == 0 {
                    break;
                }
                position -= 1;
                while position > 0 && !text.is_char_boundary(position) {
                    position -= 1;
                }
            }
        }
        std::cmp::Ordering::Equal => {}
    }
    position
}
