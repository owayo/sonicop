//! `Layout/SpaceInsideRangeLiteral`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

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
            .find(|child| matches!(child.kind(), ".." | "..."))
        else {
            continue;
        };
        let operator = &text[operator.byte_range()];
        let source = &text[node.byte_range()];
        // The cop works on the literal's text rather than on its parts, so a multiline range is
        // first folded back onto one line and only then measured.
        let expression = collapse_line_break(source, operator);
        if !spaced(&expression, operator) {
            continue;
        }
        let replacement = trim_before(&expression, operator);
        let replacement = trim_after(&replacement, operator);
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement,
            safe: true,
        }));
    }
}

/// `Builders::Default#check_condition`: a range written as a condition is a flip-flop rather than a
/// range literal, so `on_irange` and `on_erange` never see it. The rewrite reaches through a
/// parenthesized single statement, through `and` and `or`, and through the argument of `!`.
fn is_flip_flop(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let text = context.source.text();
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "parenthesized_statements" => {
                let mut cursor = parent.walk();
                if parent
                    .named_children(&mut cursor)
                    .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
                    .count()
                    != 1
                {
                    return false;
                }
            }
            "binary"
                if parent
                    .child_by_field_name("operator")
                    .is_some_and(|operator| {
                        matches!(&text[operator.byte_range()], "&&" | "||" | "and" | "or")
                    }) => {}
            "unary" => {
                return parent
                    .child(0)
                    .is_some_and(|operator| matches!(&text[operator.byte_range()], "!" | "not"));
            }
            "if" | "elsif" | "unless" | "while" | "until" | "if_modifier" | "unless_modifier"
            | "while_modifier" | "until_modifier" | "conditional" => {
                return parent.child_by_field_name("condition") == Some(current);
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
