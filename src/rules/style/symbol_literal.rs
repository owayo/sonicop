//! `Style/SymbolLiteral`: a symbol whose name needs no quoting should not carry any.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Do not use strings for word-like symbol literals.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("delimited_symbol") {
        let text = context.source.node_text(node);
        if !is_word_like(text) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: text.replace(['\'', '"'], ""),
            safe: true,
        }));
    }
}

/// `/\A:["'][A-Za-z_]\w*["']\z/`.
fn is_word_like(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(':') else {
        return false;
    };
    let mut characters = rest.chars();
    let Some(quote) = characters
        .next()
        .filter(|first| matches!(first, '"' | '\''))
    else {
        return false;
    };
    let Some(name) = characters.as_str().strip_suffix(quote) else {
        return false;
    };
    let mut letters = name.chars();
    letters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && letters.all(|letter| letter.is_ascii_alphanumeric() || letter == '_')
}
