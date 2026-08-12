//! `Layout/MultilineMethodDefinitionBraceLayout`.

use super::multiline_brace::{Literal, Messages, check_brace_layout, delimiters, grouped_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MESSAGES: Messages = Messages {
    same_line: "Closing method definition brace must be on the same line as the last parameter \
                when opening brace is on the same line as the first parameter.",
    new_line: "Closing method definition brace must be on the line after the last parameter when \
               opening brace is on a separate line from the first parameter.",
    always_new_line: "Closing method definition brace must be on the line after the last parameter.",
    always_same_line: "Closing method definition brace must be on the same line as the last \
                       parameter.",
};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // The cop reports on `node.arguments` rather than on the definition, so the parameter list is
    // the literal and the parentheses around it are its braces.
    for node in context.nodes_of("method_parameters") {
        let Some((open, close)) = delimiters(node, &["("]) else {
            continue;
        };
        check_brace_layout(
            context,
            offenses,
            &Literal {
                node,
                open,
                close,
                elements: grouped_elements(node),
            },
            &MESSAGES,
        );
    }
}
