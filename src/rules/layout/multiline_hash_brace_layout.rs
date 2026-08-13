//! `Layout/MultilineHashBraceLayout`.

use super::multiline_brace::{Literal, Messages, check_brace_layout, delimiters};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MESSAGES: Messages = Messages {
    same_line: "Closing hash brace must be on the same line as the last hash element when opening \
                brace is on the same line as the first hash element.",
    new_line: "Closing hash brace must be on the line after the last hash element when opening \
               brace is on a separate line from the first hash element.",
    always_new_line: "Closing hash brace must be on the line after the last hash element.",
    always_same_line: "Closing hash brace must be on the same line as the last hash element.",
};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // A brace-less hash never reaches here: the grammar leaves its pairs as siblings of whatever
    // was written before them, and upstream would call it an implicit literal anyway.
    for node in context.nodes_of("hash") {
        let Some((open, close)) = delimiters(node, &["{"]) else {
            continue;
        };
        let mut cursor = node.walk();
        // Each pair is a child of its own here: only a *brace-less* run folds into one `hash`.
        let elements = node
            .named_children(&mut cursor)
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .map(|child| vec![child])
            .collect();
        check_brace_layout(
            context,
            offenses,
            &Literal {
                node,
                open,
                close,
                elements,
            },
            &MESSAGES,
        );
    }
}
