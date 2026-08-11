use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "single_quotes".to_owned());
    for node in context.nodes_of("string") {
        if inside_interpolation(node) || quoted_label_key(node, context) {
            continue;
        }
        let text = context.source.node_text(node);
        let (from, to, message) = if style == "single_quotes" {
            (
                '"',
                '\'',
                "Prefer single-quoted strings when you don't need string interpolation or special symbols.",
            )
        } else {
            (
                '\'',
                '"',
                "Prefer double-quoted strings unless you need single quotes to avoid extra backslashes for escaping.",
            )
        };
        if !text.starts_with(from) || !text.ends_with(from) || text.len() < 2 {
            continue;
        }
        let content = &text[1..text.len() - 1];
        if content.contains(to)
            || content.contains('\\')
            || content.contains("#{")
            || content.contains('\n')
        {
            continue;
        }
        let replacement = format!("{to}{content}{to}");
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

fn inside_interpolation(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if parent.kind() == "interpolation" {
            return true;
        }
        node = parent;
    }
    false
}

/// A quoted hash key such as `'a': 1` is a symbol rather than a string, so re-quoting it would
/// change what it means.
fn quoted_label_key(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == "pair"
        && parent
            .child_by_field_name("key")
            .is_some_and(|key| key.byte_range() == node.byte_range())
        && context.source.text().as_bytes().get(node.end_byte()) == Some(&b':')
}
