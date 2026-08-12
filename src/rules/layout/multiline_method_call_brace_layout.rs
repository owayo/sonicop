//! `Layout/MultilineMethodCallBraceLayout`.

use super::multiline_brace::{Literal, Messages, check_brace_layout, delimiters, grouped_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MESSAGES: Messages = Messages {
    same_line: "Closing method call brace must be on the same line as the last argument when \
                opening brace is on the same line as the first argument.",
    new_line: "Closing method call brace must be on the line after the last argument when opening \
               brace is on a separate line from the first argument.",
    always_new_line: "Closing method call brace must be on the line after the last argument.",
    always_same_line: "Closing method call brace must be on the same line as the last argument.",
};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `super(...)` is a node of its own upstream rather than a `send`, so `on_send` never sees
        // it. The grammar files it under `call` and marks the keyword, which is what tells it apart
        // from the `foo.super(...)` that really is a call.
        if node
            .child_by_field_name("method")
            .is_some_and(|method| method.kind() == "super")
        {
            continue;
        }
        // An index read is a `send` upstream as well, but the parser gives it no `begin` and `end`
        // to report, so it is an implicit literal there.
        let Some(list) = node.child_by_field_name("arguments") else {
            continue;
        };
        let Some((open, close)) = delimiters(list, &["("]) else {
            continue;
        };
        check_brace_layout(
            context,
            offenses,
            &Literal {
                node,
                open,
                close,
                elements: grouped_elements(list),
            },
            &MESSAGES,
        );
    }
}
