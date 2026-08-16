//! `Layout/SpaceBeforeBlockBraces`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::final_pos;
use crate::rules::node_ext::NodeExt;

const MISSING_MSG: &str = "Space missing to the left of {.";
const DETECTED_MSG: &str = "Space detected to the left of {.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "space".to_owned());
    // `style_for_empty_braces`: an unset parameter falls back to the cop's main style.
    let empty_style: String = context
        .setting("EnforcedStyleForEmptyBraces")
        .unwrap_or_else(|| style.clone());
    let line_count_based = context
        .setting_of::<String>("Style/BlockDelimiters", "EnforcedStyle")
        .is_some_and(|delimiters| delimiters == "line_count_based");

    let text = context.source.text();
    for node in context.nodes_of("block") {
        let (Some(left), Some(right)) = (child_of_kind(node, "{"), child_of_kind(node, "}")) else {
            continue;
        };
        // `BlockNode#single_line?` compares the braces rather than the whole expression, so a block
        // opened on the last line of a multiline receiver counts as single-line.
        let multiline = left.start_position().row != right.start_position().row;
        // Correcting a multiline `no_space` block would fight `Style/BlockDelimiters`.
        if line_count_based && style == "no_space" && multiline {
            continue;
        }

        let space_start = final_pos(text, left.start_byte(), false, false, true, false);
        let used_space = space_start != left.start_byte();
        let empty = left.end_byte() == right.start_byte();
        let wanted_space = if empty {
            empty_style == "space"
        } else {
            style == "space"
        };
        if used_space == wanted_space {
            continue;
        }

        let range = if wanted_space {
            left.byte_range()
        } else {
            space_start..left.start_byte()
        };
        let message = if wanted_space {
            MISSING_MSG
        } else {
            DETECTED_MSG
        };
        // `autocorrect` reads the reported range back: blanks are dropped, anything else gains a
        // space in front of it.
        let edit = if text[range.clone()].contains(char::is_whitespace) {
            Edit {
                start: range.start,
                end: range.end,
                replacement: String::new(),
                safe: true,
            }
        } else {
            Edit {
                start: range.start,
                end: range.start,
                replacement: " ".to_owned(),
                safe: true,
            }
        };
        offenses.push(context.offense(message, range).corrected_by(edit));
    }
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == kind)
}
