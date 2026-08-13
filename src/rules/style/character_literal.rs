//! `Style/CharacterLiteral`: `?a` is a one-character string written a way nothing else is.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not use the character literal - use string literal instead.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("character") {
        let source = context.source.node_text(node);
        // `node.source.size.between?(2, 3)`: a longer literal spells an escape this cannot rewrite.
        if !(2..=3).contains(&source.chars().count()) {
            continue;
        }
        // `on_regexp` ignores the node it is given, so a character literal inside one is skipped.
        if is_inside_regexp(node) {
            continue;
        }
        let text = &source[1..];
        let replacement = if text.chars().count() == 2 || text == "'" {
            Some(format!("\"{text}\""))
        } else if text.chars().count() == 1 {
            Some(format!("'{text}'"))
        } else {
            None
        };
        let offense = context.offense(MSG, node.byte_range());
        offenses.push(match replacement {
            Some(replacement) => offense.corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
            None => offense,
        });
    }
}

fn is_inside_regexp(node: tree_sitter::Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind_str() == "regex" {
            return true;
        }
        current = parent.parent();
    }
    false
}
