use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Use `Kernel#loop` with `break` rather than `begin/end/until`(or `while`).";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["while_modifier", "until_modifier"]) {
        // `on_while_post` / `on_until_post`: the loop runs once before the condition is read only
        // when the body is a `begin ... end`. Everything else is an ordinary modifier.
        let (Some(body), Some(condition)) = (
            node.child_by_field_name("body"),
            node.child_by_field_name("condition"),
        ) else {
            continue;
        };
        if body.kind() != "begin" {
            continue;
        }
        let is_while = node.kind() == "while_modifier";
        let keyword = match keyword_token(node, if is_while { "while" } else { "until" }) {
            Some(keyword) => keyword,
            None => continue,
        };
        let last = u32::try_from(body.child_count())
            .unwrap_or(0)
            .saturating_sub(1);
        let (Some(open), Some(close)) = (body.child(0), body.child(last)) else {
            continue;
        };
        if open.kind() != "begin" || close.kind() != "end" {
            continue;
        }
        let (_, column) = context.source.line_column(node.start_byte());
        let break_line = format!(
            "break {} {}\n{}",
            if is_while { "unless" } else { "if" },
            context.source.node_text(condition),
            " ".repeat(column - 1)
        );
        offenses.push(
            context
                .offense(MSG, keyword.byte_range())
                .corrections_anchored_at(close.byte_range())
                .corrected_by_all([
                    Edit {
                        start: open.start_byte(),
                        end: open.end_byte(),
                        replacement: "loop do".to_owned(),
                        safe: true,
                    },
                    // `keyword_and_condition_range`: everything after the body's `end`.
                    Edit {
                        start: close.end_byte(),
                        end: node.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: close.start_byte(),
                        end: close.start_byte(),
                        replacement: break_line,
                        safe: true,
                    },
                ]),
        );
    }
}

fn keyword_token<'tree>(node: Node<'tree>, keyword: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.kind() == keyword)
}
