//! `MultilineLiteralBraceLayout` and the `MultilineLiteralBraceCorrector` it corrects with, which
//! four Layout cops share whole.
//!
//! Each of those cops differs only in the node it looks at, what it calls that node's elements, and
//! the four sentences it reports with; the decision and the rewrite are the same for all of them.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

/// The four sentences one of these cops reports with.
pub(super) struct Messages {
    pub(super) same_line: &'static str,
    pub(super) new_line: &'static str,
    pub(super) always_new_line: &'static str,
    pub(super) always_same_line: &'static str,
}

/// A literal as `MultilineLiteralBraceLayout` sees it: the delimiters it was written with and the
/// elements between them.
pub(super) struct Literal<'tree> {
    /// The node upstream reports the literal as, which is what its parent is looked up through.
    pub(super) node: Node<'tree>,
    /// `node.loc.begin`.
    pub(super) open: Node<'tree>,
    /// `node.loc.end`.
    pub(super) close: Node<'tree>,
    /// The elements, each as the run of nodes upstream's parser folds into one child -- a run of
    /// `key: value` pairs written without braces is a single `hash` there.
    pub(super) elements: Vec<Vec<Node<'tree>>>,
}

impl Literal<'_> {
    /// Where the first element begins. A word of a `%w` list written with a backslash escape takes
    /// the blanks before it into its own span here, while upstream's parser starts the string at the
    /// backslash, so the blanks have to be stepped over to compare the right lines.
    fn first_element_start(&self, text: &str) -> usize {
        let node = self.elements[0][0];
        let start = node.start_byte();
        let blanks = text[node.byte_range()].len() - text[node.byte_range()].trim_start().len();
        start + blanks
    }

    /// `children(node).last.source_range.end_pos`.
    fn last_element_end(&self) -> usize {
        let last = &self.elements[self.elements.len() - 1];
        last[last.len() - 1].end_byte()
    }
}

/// `check_brace_layout`, then `check` for the configured style.
pub(super) fn check_brace_layout(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    literal: &Literal<'_>,
    messages: &Messages,
) {
    // `ignored_literal?`: an implicit literal has no brace to report, and one written on a single
    // line has nothing to align. A cop over a call tests the two braces rather than the whole node,
    // but a call whose braces share a line is on a single line either way.
    if literal.elements.is_empty()
        || line(context, literal.open.start_byte()) == line_of_close(context, literal)
    {
        return;
    }
    // A heredoc reaching past the last element cannot have the brace moved above it without
    // breaking the code.
    if last_line_heredoc(context, literal) {
        return;
    }

    let opening_on_same_line = line(context, literal.open.start_byte())
        == line(context, literal.first_element_start(context.source.text()));
    let closing_on_same_line =
        line_of_close(context, literal) == last_element_line(context, literal);

    let message = match context
        .setting::<String>("EnforcedStyle")
        .as_deref()
        .unwrap_or("symmetrical")
    {
        "new_line" => closing_on_same_line.then_some(messages.always_new_line),
        "same_line" => (!closing_on_same_line).then_some(messages.always_same_line),
        _ if opening_on_same_line => (!closing_on_same_line).then_some(messages.same_line),
        _ => closing_on_same_line.then_some(messages.new_line),
    };
    let Some(message) = message else {
        return;
    };

    let offense = context.offense(message, literal.close.byte_range());
    offenses.push(correct(context, literal, offense, closing_on_same_line));
}

/// `MultilineLiteralBraceCorrector#call`.
fn correct(
    context: &RuleContext<'_>,
    literal: &Literal<'_>,
    offense: Offense,
    closing_on_same_line: bool,
) -> Offense {
    let text = context.source.text();
    if closing_on_same_line {
        // `insert_before(node.loc.end, "\n")`, whose range is the brace the offense reports.
        return offense.corrected_by(Edit {
            start: literal.close.start_byte(),
            end: literal.close.start_byte(),
            replacement: "\n".to_owned(),
            safe: true,
        });
    }

    // A comment right before the closing brace makes the move a judgement call, so the offense is
    // reported and left alone.
    let end_offset = element_end_with_trailing_comma(text, literal);
    if new_line_needed_before_closing_brace(context, literal, end_offset) {
        return offense;
    }

    let mut edits = Vec::new();
    // `correct_heredoc_argument_method_chain` inserts *after* the same empty range the brace is
    // inserted before. An `Edit` only carries an offset, so both land as insertions on that range
    // and the later one wraps around the earlier -- emitting the chain first is what puts the brace
    // in front of it, which is the order upstream's `insert_before` / `insert_after` pair produces.
    if let Some(chain) = heredoc_argument_method_chain(literal) {
        edits.push(Edit {
            start: chain.start,
            end: chain.end,
            replacement: String::new(),
            safe: true,
        });
        edits.push(Edit {
            start: end_offset,
            end: end_offset,
            replacement: text[chain].to_owned(),
            safe: true,
        });
    }
    let removed = space_before(text, literal.close.start_byte())..literal.close.end_byte();
    edits.push(Edit {
        start: removed.start,
        end: removed.end,
        replacement: String::new(),
        safe: true,
    });
    let content = match comment_at_line(context, last_element_line(context, literal)) {
        // The brace is not alone on its line, so everything after it moves up with it.
        true => {
            let trailing = literal.close.start_byte()..line_end(context, literal.close.end_byte());
            let content = text[trailing.clone()].to_owned();
            edits.push(Edit {
                start: trailing.start,
                end: trailing.end,
                replacement: String::new(),
                safe: true,
            });
            content
        }
        false => text[literal.close.byte_range()].to_owned(),
    };
    edits.push(Edit {
        start: end_offset,
        end: end_offset,
        replacement: content,
        safe: true,
    });
    offense.corrected_by_all(edits)
}

/// `new_line_needed_before_closing_brace?`.
fn new_line_needed_before_closing_brace(
    context: &RuleContext<'_>,
    literal: &Literal<'_>,
    end_offset: usize,
) -> bool {
    comment_at_line(context, line(context, end_offset))
        && (is_chained(context, literal.node) || is_argument(context, literal.node))
}

/// `last_element_range_with_trailing_comma(node).end`, as the offset that range closes at.
fn element_end_with_trailing_comma(text: &str, literal: &Literal<'_>) -> usize {
    let end = literal.last_element_end();
    let after = space_after(text, end);
    match text.as_bytes().get(after) {
        Some(b',') => after + 1,
        _ => end,
    }
}

/// `correct_heredoc_argument_method_chain`: the `.foo` a call carrying a heredoc as its first
/// argument is chained onto, which has to travel with the brace.
fn heredoc_argument_method_chain(literal: &Literal<'_>) -> Option<Range<usize>> {
    // `node.respond_to?(:first_argument)`: only a call has arguments to look at.
    if literal.node.kind_str() != "call" {
        return None;
    }
    let first = literal.elements[0][0];
    if first.kind_str() != "heredoc_beginning" {
        return None;
    }
    let parent = literal.node.parent()?;
    if parent.kind_str() != "call" {
        return None;
    }
    // Upstream reads `parent.loc.dot` without testing it, and raises on a call written without one.
    let dot = parent.field("operator")?;
    Some(dot.start_byte()..parent.end_byte())
}

/// `last_line_heredoc?(node.children.last)`: the last element holds a heredoc whose terminator
/// reaches the element's own last line or beyond.
fn last_line_heredoc(context: &RuleContext<'_>, literal: &Literal<'_>) -> bool {
    let last = &literal.elements[literal.elements.len() - 1];
    let span = last[0].start_byte()..last[last.len() - 1].end_byte();
    let parent_line = last_element_line(context, literal);
    super::support::heredoc_terminators(context)
        .into_iter()
        .any(|(opener, terminator)| {
            span.contains(&opener) && line(context, terminator.start) >= parent_line
        })
}

/// `processed_source.comment_at_line`.
fn comment_at_line(context: &RuleContext<'_>, target: usize) -> bool {
    context
        .comment_ranges()
        .iter()
        .any(|comment| line(context, comment.start) == target)
}

fn line(context: &RuleContext<'_>, offset: usize) -> usize {
    context.source.line_column(offset).0
}

fn line_of_close(context: &RuleContext<'_>, literal: &Literal<'_>) -> usize {
    line(context, literal.close.start_byte())
}

/// `children(node).last.last_line`.
fn last_element_line(context: &RuleContext<'_>, literal: &Literal<'_>) -> usize {
    line(context, literal.last_element_end())
}

/// The end of the line `offset` sits on, before its line break.
fn line_end(context: &RuleContext<'_>, offset: usize) -> usize {
    let row = line(context, offset);
    let range = context.source.line_range(row);
    range.start
        + context
            .source
            .line(row)
            .trim_end_matches(['\n', '\r'])
            .len()
}

/// `range_with_surrounding_space(range, side: :left)`: the blanks before `offset`, then the line
/// breaks before those -- and no further, so a blank line above is left alone.
fn space_before(text: &str, offset: usize) -> usize {
    support::final_pos(text, offset, false, false, true, false)
}

/// `range_with_surrounding_space(range, side: :right)`.
fn space_after(text: &str, offset: usize) -> usize {
    support::final_pos(text, offset, true, false, true, false)
}

/// `Node#chained?`: the node is the receiver of the call written around it.
fn is_chained(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    match parent.kind_str() {
        "call" => parent.field("receiver") == Some(node),
        // An index read is a `send` upstream, so its object is a receiver like any other.
        "element_reference" => parent.field("object") == Some(node),
        "binary" => {
            super::support::is_send_like(context, parent) && parent.field("left") == Some(node)
        }
        "unary" => super::support::is_send_like(context, parent),
        _ => false,
    }
}

/// `Node#argument?`: the node is an argument of the `send` written around it. A `csend` is not a
/// `send`, so an argument of `foo&.bar(...)` answers no.
fn is_argument(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    match parent.kind_str() {
        "argument_list" => parent.parent_of(context).is_some_and(|call| {
            call.kind_str() == "call" && crate::rules::send_node::is_plain_send(call, context)
        }),
        "element_reference" => parent.field("object") != Some(node),
        "binary" => {
            super::support::is_send_like(context, parent) && parent.field("right") == Some(node)
        }
        // `a.b = value` and `a[0] = value` are both `send` calls whose last argument is the value.
        "assignment" | "operator_assignment" => {
            parent.field("right") == Some(node)
                && parent
                    .field("left")
                    .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"))
        }
        _ => false,
    }
}

/// The elements of a container, as upstream's parser groups them: a trailing run of `key: value`
/// pairs and `**splat`s is one `hash` child there however many pairs were written.
pub(super) fn grouped_elements<'tree>(container: Node<'tree>) -> Vec<Vec<Node<'tree>>> {
    let mut cursor = container.walk();
    let children: Vec<Node<'tree>> = container
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect();
    let mut elements: Vec<Vec<Node<'tree>>> = Vec::new();
    for child in children {
        let pair = matches!(child.kind_str(), "pair" | "hash_splat_argument");
        match elements.last_mut() {
            Some(last) if pair && matches!(last[0].kind_str(), "pair" | "hash_splat_argument") => {
                last.push(child);
            }
            _ => elements.push(vec![child]),
        }
    }
    elements
}

/// `node.loc.begin` and `node.loc.end`: the delimiters a literal was written with, when it was
/// written with any. A literal without them -- `foo a, b`, `return 1, 2`, `def foo x` -- is the
/// implicit literal these cops leave alone.
pub(super) fn delimiters<'tree>(
    node: Node<'tree>,
    openers: &[&str],
) -> Option<(Node<'tree>, Node<'tree>)> {
    let open = node
        .child(0)
        .filter(|child| openers.contains(&child.kind_str()))?;
    let last = u32::try_from(node.child_count()).ok()?.checked_sub(1)?;
    let close = node
        .child(last)
        .filter(|child| child.start_byte() > open.start_byte())?;
    Some((open, close))
}
