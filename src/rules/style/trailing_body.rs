//! `TrailingBody` and `LineBreakCorrector`, shared by the cops that push a body onto its own line.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Reports `node`'s body when it starts on the line the definition opened on.
pub(super) fn check(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    message: &str,
) {
    let Some(body) = node.field("body") else {
        return;
    };
    let statements = super::nodes::children(body);
    let (Some(first), Some(last)) = (statements.first(), statements.last()) else {
        return;
    };
    // `node.multiline? && same_line?(node, body)`.
    if node.start_position().row == node.end_position().row
        || node.start_position().row != first.start_position().row
    {
        return;
    }
    // `first_part_of`: the first statement, except that a body carrying a `rescue` or an `ensure`
    // is one node upstream and reports whole.
    let clause = statements
        .iter()
        .any(|child| matches!(child.kind_str(), "rescue" | "ensure" | "else"));
    let range = match clause {
        true => first.start_byte()..last.end_byte(),
        false => first.byte_range(),
    };

    let width: usize = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2)
        .max(0) as usize;
    let column = node.start_position().column;

    let mut edits = vec![Edit {
        start: range.start,
        end: range.start,
        replacement: format!("\n{}", " ".repeat(column + width)),
        safe: true,
    }];
    // `move_comment`: a comment closing the opening line has to move above the definition, since
    // everything else on that line is about to move below it.
    if let Some(comment) = comment_on_line(context, node.start_position().row) {
        edits.push(Edit {
            start: node.start_byte(),
            end: node.start_byte(),
            replacement: format!(
                "{}\n{}",
                &context.source.text()[comment.clone()],
                " ".repeat(column)
            ),
            safe: true,
        });
        edits.push(Edit {
            start: comment.start,
            end: comment.end,
            replacement: String::new(),
            safe: true,
        });
    }
    // `remove_semicolon`: the `;` that used to separate the signature from the body.
    if let Some(semicolon) = trailing_semicolon(context, node, *first) {
        edits.push(Edit {
            start: semicolon,
            end: semicolon + 1,
            replacement: String::new(),
            safe: true,
        });
    }
    offenses.push(context.offense(message, range).corrected_by_all(edits));
}

fn comment_on_line(context: &RuleContext<'_>, row: usize) -> Option<Range<usize>> {
    context
        .comment_ranges()
        .iter()
        .find(|range| context.source.line_column(range.start).0 == row + 1)
        .cloned()
}

/// The first `;` after the definition opened that sits on the body's line, to the left of it.
fn trailing_semicolon(context: &RuleContext<'_>, node: Node<'_>, body: Node<'_>) -> Option<usize> {
    let text = context.source.text();
    let line = context.source.line_range(body.start_position().row + 1);
    let from = line.start.max(node.start_byte());
    text[from..body.start_byte()]
        .char_indices()
        .find(|(_, character)| *character == ';')
        .map(|(offset, _)| from + offset)
}
