//! `Layout/SpaceAfterSemicolon`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    let space_before_rcurly = context
        .setting_of::<String>("Layout/SpaceInsideBlockBraces", "EnforcedStyle")
        .as_deref()
        .unwrap_or("space")
        != "no_space";
    // The `}` that closes an interpolation is a `tSTRING_DEND` rather than a `tRCURLY`, and never
    // wants a space in front of it.
    let interpolation_ends: Vec<usize> = context
        .nodes_of("interpolation")
        .map(|node| node.end_byte())
        .collect();

    for node in context.nodes() {
        if node.kind() != ";" {
            continue;
        }
        let after = node.end_byte();
        let Some(next) = text.as_bytes().get(after) else {
            continue;
        };
        // `space_missing?`: the next token opens in the very next column.
        if next.is_ascii_whitespace() {
            continue;
        }
        // `semicolon_sequence?`, then `allowed_type?`.
        if matches!(next, b';' | b')' | b']' | b'|') {
            continue;
        }
        if *next == b'}'
            && (interpolation_ends.contains(&(after + 1)) || !space_before_rcurly)
        {
            continue;
        }
        offenses.push(
            context
                .offense("Space missing after semicolon.", node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: after,
                    replacement: "; ".to_owned(),
                    safe: true,
                }),
        );
    }
}
