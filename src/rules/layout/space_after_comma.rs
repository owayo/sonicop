//! `Layout/SpaceAfterComma`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let bytes = context.source.text().as_bytes();
    // RuboCop lets a comma sit against `}` only where `Layout/SpaceInsideHashLiteralBraces`
    // forbids the space that would otherwise follow it.
    let no_space_before_rcurly = context
        .setting_of::<String>("Layout/SpaceInsideHashLiteralBraces", "EnforcedStyle")
        .as_deref()
        == Some("no_space");
    for node in context.nodes() {
        // RuboCop walks the lexer's tokens, where a comma inside a string literal or a heredoc
        // delimiter is part of that literal rather than a comma. The tree has the same
        // distinction: only a comma the parser recognised is a node of its own.
        if node.kind_str() != "," {
            continue;
        }
        let index = node.start_byte();
        let Some(&next) = bytes.get(index + 1) else {
            continue;
        };
        // The offense is about the *next token* starting one column along, so anything but a
        // token butting against the comma -- whitespace, a line break, the end of the file --
        // leaves nothing to report.
        let skip = match next {
            b' ' | b'\t' | b'\r' | b'\n' => true,
            // `;` is not a comma's business, and `)`, `]` and `|` close a construct where
            // RuboCop asks for no space.
            b';' | b')' | b']' | b'|' => true,
            b'}' => no_space_before_rcurly,
            _ => false,
        };
        if skip {
            continue;
        }
        offenses.push(
            context
                .offense("Space missing after comma.", index..index + 1)
                .corrected_by(Edit {
                    start: index + 1,
                    end: index + 1,
                    replacement: " ".to_owned(),
                    safe: true,
                }),
        );
    }
}
