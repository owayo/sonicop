//! Line counting shared by the length cops.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::{RuleContext, push_named_children, walk_named};

/// What kind of construct a length cop measures. The three differ in how the body is counted and
/// where the offense is reported, so naming the kind keeps those differences in one place instead
/// of spreading cop-name comparisons through the counting code.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum LengthTarget {
    /// A method, counted over its body.
    Method,
    /// A class or module, counted over its interior with nested classes and modules removed.
    Classlike,
    /// A block, reported against the call that owns it.
    Block,
}

/// Where each heredoc's terminator sits, keyed by the offset of the `<<~FOO` that opened it.
///
/// RuboCop's AST gives a heredoc node the range of its opener alone, and `CodeLengthCalculator`
/// reaches past that to `loc.heredoc_end` when a body holds one. tree-sitter instead hangs the
/// content off a `heredoc_body` sibling, so the two have to be paired back up. Openers and bodies
/// both appear in source order and Ruby stacks them in that same order, which is what makes
/// pairing them by rank correct.
pub(super) struct HeredocEnds(HashMap<usize, usize>);

impl HeredocEnds {
    pub(super) fn new(context: &RuleContext<'_>) -> Self {
        let bodies: Vec<Node<'_>> = context.nodes_of("heredoc_body").collect();
        Self(
            context
                .nodes_of("heredoc_beginning")
                .zip(bodies)
                .map(|(opener, body)| (opener.start_byte(), body.end_position().row))
                .collect(),
        )
    }

    fn end_row(&self, opener: Node<'_>) -> usize {
        self.0
            .get(&opener.start_byte())
            .copied()
            .unwrap_or_else(|| opener.end_position().row)
    }
}

/// Reports `node` when it holds more than `max` lines of code, in the shape RuboCop's length cops
/// use: `Method has too many lines. [12/10]`.
pub(super) fn report_length(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    max: usize,
    label: &str,
    target: LengthTarget,
    heredocs: &HeredocEnds,
) {
    let count_comments: bool = context.setting("CountComments").unwrap_or(false);
    if node.child_by_field_name("body").is_none() {
        return;
    }
    let length = if target == LengthTarget::Classlike {
        classlike_code_line_count(node, context, count_comments)
    } else {
        body_code_line_count(node, context, count_comments, heredocs)
    };
    if length <= max {
        return;
    }
    let location = if target == LengthTarget::Block {
        block_location(node)
    } else {
        node
    };
    offenses.push(context.offense(
        format!("{label} has too many lines. [{length}/{max}]"),
        location.byte_range(),
    ));
}

/// The node a block's offense is reported against: RuboCop's `block` node starts at the call that
/// takes the block, or at the `->` of a lambda literal, never at the brace.
pub(super) fn block_location<'tree>(node: Node<'tree>) -> Node<'tree> {
    node.parent()
        .filter(|parent| matches!(parent.kind(), "call" | "lambda"))
        .unwrap_or(node)
}

/// The lines RuboCop counts for the body of a method or block.
///
/// RuboCop takes the *source of the body node* rather than the span of the enclosing definition,
/// and switches to whole source lines only when the body holds a heredoc. The two differ at both
/// ends, which is why the distinction is worth reproducing rather than approximating: a body that
/// is nothing but a heredoc measures one line (its opener), while a body whose last statement runs
/// past a heredoc terminator has to be followed out to that terminator.
fn body_code_line_count(
    node: Node<'_>,
    context: &RuleContext<'_>,
    count_comments: bool,
    heredocs: &HeredocEnds,
) -> usize {
    let Some(body) = node.child_by_field_name("body") else {
        return 0;
    };
    let statements = statements_of(body);
    let (Some(first), Some(last)) = (statements.first(), statements.last()) else {
        return 0;
    };
    let start = first.start_position().row;
    let end = heredoc_extended_end(&statements, heredocs).unwrap_or(last.end_position().row);
    count_code_lines(context, start, end, count_comments)
}

/// The statements RuboCop would see as the body. A `heredoc_body` is content, not a statement:
/// tree-sitter parks it beside the statement that opened it, where RuboCop has nothing at all.
fn statements_of(body: Node<'_>) -> Vec<Node<'_>> {
    if !matches!(body.kind(), "body_statement" | "block_body") {
        return vec![body];
    }
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|child| child.kind() != "heredoc_body")
        .collect()
}

/// The last line touched by anything *inside* the body, with heredocs followed out to their
/// terminator -- `None` when the body holds no heredoc and the plain body range applies.
///
/// A single statement stands in for RuboCop's body node itself, so only its descendants count and
/// its own closing `end` drops out of the span. Several statements are wrapped in a `begin` whose
/// descendants include the statements themselves, so those do count.
fn heredoc_extended_end(statements: &[Node<'_>], heredocs: &HeredocEnds) -> Option<usize> {
    let mut stack = Vec::new();
    if let [only] = statements {
        push_named_children(*only, &mut stack);
    } else {
        stack.extend(statements.iter().copied());
    }
    let mut found = false;
    let mut last_row = 0;
    while let Some(current) = stack.pop() {
        let row = if current.kind() == "heredoc_beginning" {
            found = true;
            heredocs.end_row(current)
        } else {
            current.end_position().row
        };
        last_row = last_row.max(row);
        push_named_children(current, &mut stack);
    }
    found.then_some(last_row)
}

fn count_code_lines(
    context: &RuleContext<'_>,
    start_row: usize,
    end_row: usize,
    count_comments: bool,
) -> usize {
    (start_row..=end_row)
        .filter(|row| {
            let text = context.source.line(row + 1).trim();
            !text.is_empty() && (count_comments || !text.starts_with('#'))
        })
        .count()
}

fn classlike_code_line_count(
    node: Node<'_>,
    context: &RuleContext<'_>,
    count_comments: bool,
) -> usize {
    let mut excluded_lines = HashSet::new();
    walk_named(node, &mut |descendant| {
        if descendant == node || !matches!(descendant.kind(), "class" | "module") {
            return;
        }
        let first = descendant.start_position().row + 1;
        let last = descendant.end_position().row + 1;
        excluded_lines.extend(first..=last);
    });

    // RuboCop's ProcessedSource is indexed from zero after constructing the
    // one-based interior line range. Preserve that observable offset exactly.
    let start = node.start_position().row + 2;
    let end = node.end_position().row;
    (start..=end)
        .filter(|line| {
            if excluded_lines.contains(line) {
                return false;
            }
            let text = context.source.line(*line + 1).trim();
            !text.is_empty() && (count_comments || !text.starts_with('#'))
        })
        .count()
}
