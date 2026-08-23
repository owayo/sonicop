//! `Layout/SpaceInsideRangeLiteral`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Space inside range literal.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for node in context.nodes_of("range") {
        if is_flip_flop(context, node) {
            continue;
        }
        let mut cursor = node.walk();
        let Some(operator) = node
            .children(&mut cursor)
            .find(|child| matches!(child.kind_str(), ".." | "..."))
        else {
            continue;
        };
        let operator_text = &text[operator.byte_range()];
        // Ruby continues a range across the line break after its operator -- `0 ..\n 10` is one
        // `irange`. The grammar stops the node at the operator and reads the rest as a statement of
        // its own, so the literal is stitched back together before it is measured.
        let end = continued_end(context, node, operator).unwrap_or_else(|| node.end_byte());
        let operator = operator_text;
        let source = &text[node.start_byte()..end];
        // The cop works on the literal's text rather than on its parts, so a multiline range is
        // first folded back onto one line and only then measured.
        let expression = collapse_line_break(source, operator);
        if !spaced(&expression, operator) {
            continue;
        }
        let replacement = trim_before(&expression, operator);
        let replacement = trim_after(&replacement, operator);
        offenses.push(
            context
                .offense(MSG, node.start_byte()..end)
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// Where the literal really ends when the grammar cut it short at a line break.
///
/// A range whose operator is its last token is either endless -- `(0..)` -- or continued on the
/// next line, and only the second joins what follows into the same node upstream. The statement the
/// grammar built out of the continuation is the sibling that follows the one holding the range.
fn continued_end(context: &RuleContext<'_>, node: Node<'_>, operator: Node<'_>) -> Option<usize> {
    if operator.end_byte() != node.end_byte() {
        return None;
    }
    if !context.source.text()[node.end_byte()..].starts_with('\n') {
        return None;
    }
    let mut current = node;
    loop {
        if let Some(next) = current.next_named_sibling() {
            return Some(next.end_byte());
        }
        let parent = current.parent()?;
        if parent.kind_str() == "program" {
            return None;
        }
        current = parent;
    }
}

/// `Builders::Default#check_condition`: a range written as a condition is a flip-flop rather than a
/// range literal, so `on_irange` and `on_erange` never see it. The rewrite reaches through a
/// parenthesized single statement, through `and` and `or`, and through the argument of `!`.
fn is_flip_flop(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let text = context.source.text();
    let mut current = node;
    while let Some(parent) = current.parent_of(context) {
        match parent.kind_str() {
            "parenthesized_statements" => {
                let mut cursor = parent.walk();
                if parent
                    .named_children(&mut cursor)
                    .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
                    .count()
                    != 1
                {
                    return false;
                }
            }
            "binary"
                if parent.field("operator").is_some_and(|operator| {
                    matches!(&text[operator.byte_range()], "&&" | "||" | "and" | "or")
                }) => {}
            "unary" => {
                return parent
                    .child(0)
                    .is_some_and(|operator| matches!(&text[operator.byte_range()], "!" | "not"));
            }
            "if" | "elsif" | "unless" | "while" | "until" | "if_modifier" | "unless_modifier"
            | "while_modifier" | "until_modifier" | "conditional" => {
                return parent.field("condition") == Some(current);
            }
            _ => return false,
        }
        current = parent;
    }
    false
}

/// `expression.sub!(/#{op}\n\s*/, op)`: the first line break written right after the operator, and
/// the indentation that follows it, are taken out before the literal is judged.
fn collapse_line_break(source: &str, operator: &str) -> String {
    let bytes = source.as_bytes();
    let operator = operator.as_bytes();
    for index in 0..bytes.len() {
        if !bytes[index..].starts_with(operator) {
            continue;
        }
        let after = index + operator.len();
        if bytes.get(after) != Some(&b'\n') {
            continue;
        }
        let mut end = after + 1;
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        return format!("{}{}", &source[..after], &source[end..]);
    }
    source.to_owned()
}

/// `/(\s#{op})|(#{op}\s)/`: blank on either side of the operator.
fn spaced(expression: &str, operator: &str) -> bool {
    let bytes = expression.as_bytes();
    let operator = operator.as_bytes();
    (0..bytes.len()).any(|index| {
        (bytes[index].is_ascii_whitespace() && bytes[index + 1..].starts_with(operator))
            || (bytes[index..].starts_with(operator)
                && bytes
                    .get(index + operator.len())
                    .is_some_and(u8::is_ascii_whitespace))
    })
}

/// `expression.sub(/\s+#{op}/, op)`.
fn trim_before(expression: &str, operator: &str) -> String {
    let bytes = expression.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        if bytes[end..].starts_with(operator.as_bytes()) {
            return format!("{}{}", &expression[..index], &expression[end..]);
        }
        index = end;
    }
    expression.to_owned()
}

/// `expression.sub(/#{op}\s+/, op)`.
fn trim_after(expression: &str, operator: &str) -> String {
    let bytes = expression.as_bytes();
    for index in 0..bytes.len() {
        if !bytes[index..].starts_with(operator.as_bytes()) {
            continue;
        }
        let start = index + operator.len();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        if end > start {
            return format!("{}{}", &expression[..start], &expression[end..]);
        }
    }
    expression.to_owned()
}
