use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::source::is_protected;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ranges = context.protected_ranges();
    let bytes = context.source.text().as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b',' || is_protected(index, ranges) {
            continue;
        }
        let next = bytes.get(index + 1).copied();
        if next.is_none_or(|byte| {
            matches!(
                byte,
                b' ' | b'\t' | b'\r' | b'\n' | b')' | b']' | b'}' | b'|'
            )
        }) {
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
