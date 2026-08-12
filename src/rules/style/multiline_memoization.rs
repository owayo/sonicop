use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const KEYWORD_MSG: &str = "Wrap multiline memoization blocks in `begin` and `end`.";
const BRACES_MSG: &str = "Wrap multiline memoization blocks in `(` and `)`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let braces = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "braces");

    for node in context.nodes_of("operator_assignment") {
        if node
            .child_by_field_name("operator")
            .is_none_or(|operator| context.source.node_text(operator) != "||=")
        {
            continue;
        }
        let Some(right) = node.child_by_field_name("right") else {
            continue;
        };
        if right.start_position().row == right.end_position().row {
            continue;
        }
        let (Some(open), Some(close)) = (
            right.child(0),
            right.child(right.child_count().saturating_sub(1) as u32),
        ) else {
            continue;
        };
        let edits = match braces {
            // `rhs.kwbegin_type?`: `begin ... end`, unless it carries a `rescue` or `ensure`,
            // which parentheses cannot hold.
            true => {
                if right.kind() != "begin" || has_rescue_or_ensure(right) {
                    continue;
                }
                vec![replacement(open, "("), replacement(close, ")")]
            }
            // `rhs.begin_type?`: a parenthesized group.
            false => {
                if right.kind() != "parenthesized_statements" {
                    continue;
                }
                vec![
                    replacement(open, &keyword_begin(context, right, open)),
                    replacement(close, &keyword_end(context, right, close)),
                ]
            }
        };
        let message = match braces {
            true => BRACES_MSG,
            false => KEYWORD_MSG,
        };
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

fn replacement(node: Node<'_>, text: &str) -> Edit {
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: text.to_owned(),
        safe: true,
    }
}

/// `keyword_begin_str`: the first statement stays where it was, so a group that opened on the same
/// line as its first statement gains a newline and the body's indentation.
fn keyword_begin(context: &RuleContext<'_>, right: Node<'_>, open: Node<'_>) -> String {
    if context.source.text().as_bytes().get(open.end_byte()) == Some(&b'\n') {
        return "begin".to_owned();
    }
    let width: usize = context
        .setting_of::<i64>("Layout/IndentationWidth", "Width")
        .unwrap_or(2)
        .max(0) as usize;
    format!(
        "begin\n{}",
        " ".repeat(right.start_position().column + width)
    )
}

/// `keyword_end_str`: a `)` sharing its line with anything but blanks is moved onto its own.
fn keyword_end(context: &RuleContext<'_>, right: Node<'_>, close: Node<'_>) -> String {
    let line = context.source.line(close.start_position().row + 1);
    if line
        .chars()
        .any(|character| !character.is_whitespace() && character != ')')
    {
        return format!("\n{}end", " ".repeat(right.start_position().column));
    }
    "end".to_owned()
}

fn has_rescue_or_ensure(node: Node<'_>) -> bool {
    super::nodes::children(node)
        .iter()
        .any(|child| matches!(child.kind(), "rescue" | "ensure"))
}
