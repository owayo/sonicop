//! `Layout/MultilineArrayBraceLayout`.

use super::multiline_brace::{Literal, Messages, check_brace_layout, delimiters, grouped_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MESSAGES: Messages = Messages {
    same_line: "The closing array brace must be on the same line as the last array element when \
                the opening brace is on the same line as the first array element.",
    new_line: "The closing array brace must be on the line after the last array element when the \
               opening brace is on a separate line from the first array element.",
    always_new_line: "The closing array brace must be on the line after the last array element.",
    always_same_line: "The closing array brace must be on the same line as the last array element.",
};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // A `%w` or `%i` list is an `array` upstream too, and its percent opener is the brace reported.
    for node in context.nodes_of_any(&["array", "string_array", "symbol_array"]) {
        let Some((open, close)) = delimiters(node, &["[", "%w(", "%i("]) else {
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
