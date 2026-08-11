use tree_sitter::Node;

use super::support::{whitespace_after, whitespace_before};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "assignment", "operator_assignment"]) {
        match node.kind() {
            "binary" => {
                let Some(operator) = node.child_by_field_name("operator") else {
                    continue;
                };
                let text = context.source.node_text(operator);
                if matches!(text, "+" | "-")
                    && node
                        .child_by_field_name("left")
                        .is_some_and(|left| matches!(left.kind(), "return" | "break" | "next"))
                {
                    continue;
                }
                let require_space = text != "**";
                check_operator(context, offenses, operator, require_space);
            }
            _ => {
                if let Some(operator) = operator_child(node) {
                    check_operator(context, offenses, operator, true);
                }
            }
        }
    }
}

fn operator_child(node: Node<'_>) -> Option<Node<'_>> {
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    let mut cursor = node.walk();
    node.children(&mut cursor).find(|child| {
        child.start_byte() >= left.end_byte() && child.end_byte() <= right.start_byte()
    })
}

fn check_operator(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    operator: Node<'_>,
    require_space: bool,
) {
    if context.in_heredoc(operator.byte_range()) {
        return;
    }
    let source = context.source.text();
    let before = whitespace_before(source, operator.start_byte());
    let after = whitespace_after(source, operator.end_byte());
    let operator_text = context.source.node_text(operator);
    if operator_text == "=" && source.as_bytes().get(operator.end_byte()) == Some(&b'~') {
        return;
    }
    let touches_line_break = operator
        .start_byte()
        .checked_sub(1)
        .and_then(|index| source.as_bytes().get(index))
        .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
        || source
            .as_bytes()
            .get(operator.end_byte())
            .is_some_and(|byte| matches!(byte, b'\r' | b'\n'));
    let alignment_allowed: bool = context.setting("AllowForAlignment").unwrap_or(true);
    if touches_line_break || (alignment_allowed && (before.len() > 1 || after.len() > 1)) {
        return;
    }
    let correct = if require_space {
        before.len() == 1
            && &source[before.clone()] == " "
            && after.len() == 1
            && &source[after.clone()] == " "
    } else {
        before.is_empty() && after.is_empty()
    };
    if correct {
        return;
    }

    let message = if require_space {
        format!("Surrounding space missing for operator `{operator_text}`.")
    } else {
        format!("Space around operator `{operator_text}` detected.")
    };
    let replacement = if require_space {
        format!(" {operator_text} ")
    } else {
        operator_text.to_owned()
    };
    offenses.push(
        context
            .offense(message, operator.start_byte()..operator.end_byte())
            .corrected_by(Edit {
                start: before.start,
                end: after.end,
                replacement,
                safe: true,
            }),
    );
}
