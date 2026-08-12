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
        let (Some(name), Some(value)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ) else {
            continue;
        };
        for (name_end, value_start) in optional_parameters(name, value) {
            let Some(equals) = text[name_end..value_start]
                .find('=')
                .map(|index| name_end + index)
            else {
                continue;
            };
            let spaced = |offset: usize| text[offset..].starts_with(char::is_whitespace);
            let both = spaced(name_end) && spaced(equals + 1);
            let neither = !spaced(name_end) && !spaced(equals + 1);
            if (style == "space" && both) || (style == "no_space" && neither) {
                continue;
            }
            let range = name_end..value_start;
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
}

/// The `(name end, value start)` pairs one `optional_parameter` really holds.
///
/// A run such as `def f(a = 1, b = 2)` is several `optarg`s upstream, but the grammar folds it into
/// one parameter whose value is a right-nested multiple assignment as soon as the first default
/// could open a left-hand side. Each level of that chain contributes one more parameter: its
/// assignment list holds the previous parameter's value and the next parameter's name.
fn optional_parameters(name: Node<'_>, value: Node<'_>) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut name_end = name.end_byte();
    let mut value = value;
    loop {
        let folded = (value.kind() == "assignment")
            .then(|| value.child_by_field_name("left"))
            .flatten()
            .filter(|left| left.kind() == "left_assignment_list")
            .and_then(|left| {
                let mut cursor = left.walk();
                let parts: Vec<Node<'_>> = left
                    .named_children(&mut cursor)
                    .filter(|part| !matches!(part.kind(), "comment" | "heredoc_body"))
                    .collect();
                match (parts.len() == 2, value.child_by_field_name("right")) {
                    (true, Some(right)) => Some((parts[0], parts[1], right)),
                    _ => None,
                }
            });
        let Some((first_value, next_name, right)) = folded else {
            pairs.push((name_end, value.start_byte()));
            return pairs;
        };
        pairs.push((name_end, first_value.start_byte()));
        name_end = next_name.end_byte();
        value = right;
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
