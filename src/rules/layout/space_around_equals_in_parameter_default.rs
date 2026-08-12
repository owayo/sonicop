//! `Layout/SpaceAroundEqualsInParameterDefault`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "space".to_owned());
    let text = context.source.text();
    for node in context.nodes_of("optional_parameter") {
        let (Some(name), Some(equals), Some(value)) = (
            node.child_by_field_name("name"),
            child_of_kind(node, "="),
            node.child_by_field_name("value"),
        ) else {
            continue;
        };
        let spaced = |offset: usize| text[offset..].starts_with(char::is_whitespace);
        let both = spaced(name.end_byte()) && spaced(equals.end_byte());
        let neither = !spaced(name.end_byte()) && !spaced(equals.end_byte());
        if (style == "space" && both) || (style == "no_space" && neither) {
            continue;
        }
        let range = name.end_byte()..value.start_byte();
        let replacement = if style == "space" { " = " } else { "=" };
        let message = if style == "space" {
            "Surrounding space missing in default value assignment."
        } else {
            "Surrounding space detected in default value assignment."
        };
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: format!("{replacement}{}", rest_of(&text[range])),
            safe: true,
        }));
    }
}

/// `range.source.match(/=\s*(\S+)/)`: whatever the span still holds past the operator.
fn rest_of(source: &str) -> &str {
    let Some(index) = source.find('=') else {
        return "";
    };
    let tail = source[index + 1..].trim_start();
    match tail.find(char::is_whitespace) {
        Some(end) => &tail[..end],
        None => tail,
    }
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}
