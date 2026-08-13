//! `Layout/ConditionPosition`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // The modifier forms and the ternary have their own node kinds here, so reaching one of these
    // already means `modifier_form?` and `ternary?` are both false.
    for node in context.nodes_of_any(&["if", "unless", "while", "until"]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let Some(keyword) = node.child(0) else {
            continue;
        };
        // `single_line_condition?`: the condition opens on the keyword's own line. A condition
        // that merely runs on past it is not this cop's business.
        if keyword.start_position().row == condition.start_position().row {
            continue;
        }
        let text = context.source.text();
        let message = format!(
            "Place the condition on the same line as `{}`.",
            &text[keyword.byte_range()]
        );
        offenses.push(
            context
                .offense(message, condition.byte_range())
                .corrected_by_all([
                    Edit {
                        start: keyword.end_byte(),
                        end: keyword.end_byte(),
                        replacement: format!(" {}", &text[condition.byte_range()]),
                        safe: true,
                    },
                    removal(context, node, condition),
                ])
                // `insert_after(condition.parent.loc.keyword, ...)`: the keyword is the anchor, not
                // the condition the offense was reported on.
                .corrections_anchored_at(keyword.byte_range()),
        );
    }
}

/// `removal_range`: the condition's own lines, unless the body shares the condition's last line.
fn removal(context: &RuleContext<'_>, node: Node<'_>, condition: Node<'_>) -> Edit {
    let body = node
        .field("consequence")
        .or_else(|| node.field("body"));
    let body_start = body
        .and_then(|body| {
            let mut cursor = body.walk();
            body.named_children(&mut cursor)
                .find(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        })
        .map(|first| first.start_byte());
    if let Some(start) = body_start {
        if context.source.line_column(start).0 == condition.end_position().row + 1 {
            return Edit {
                start: condition.start_byte(),
                end: start,
                replacement: String::new(),
                safe: true,
            };
        }
    }
    let first = context.source.line_column(condition.start_byte()).0;
    let last = condition.end_position().row + 1;
    Edit {
        start: context.source.line_start(first),
        end: context.source.line_start(last + 1),
        replacement: String::new(),
        safe: true,
    }
}
