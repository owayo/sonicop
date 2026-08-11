//! Line counting shared by the length cops.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::{RuleContext, walk_named};

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

/// Reports `node` when it holds more than `max` lines of code, in the shape RuboCop's length cops
/// use: `Method has too many lines. [12/10]`.
pub(super) fn report_length(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    max: usize,
    label: &str,
    target: LengthTarget,
) {
    let count_comments: bool = context.setting("CountComments").unwrap_or(false);
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut length = if target == LengthTarget::Classlike {
        classlike_code_line_count(node, context, count_comments)
    } else {
        code_line_count(body, context, count_comments)
    };
    // A block whose whole body is one heredoc spends a line on the heredoc opener, which RuboCop
    // attributes to the enclosing statement rather than to the block.
    if target == LengthTarget::Block
        && body.named_child_count() == 1
        && context.heredoc_count(body.byte_range()) > 0
    {
        length = length.saturating_sub(1);
    }
    if length <= max {
        return;
    }
    let location = if target == LengthTarget::Block {
        node.parent()
            .filter(|parent| parent.kind() == "call")
            .unwrap_or(node)
    } else {
        node
    };
    offenses.push(context.offense(
        format!("{label} has too many lines. [{length}/{max}]"),
        location.byte_range(),
    ));
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

fn code_line_count(node: Node<'_>, context: &RuleContext<'_>, count_comments: bool) -> usize {
    let start = node.start_position().row + 1;
    let end = node.end_position().row + 1;
    (start..=end)
        .filter(|line| {
            let text = context.source.line(*line).trim();
            !text.is_empty() && (count_comments || !text.starts_with('#'))
        })
        .count()
}
