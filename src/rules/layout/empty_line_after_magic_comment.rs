use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut last_magic = None;
    for line_number in 1..=context.source.line_count().min(4) {
        let line = context.source.line(line_number).trim();
        if line_number == 1 && line.starts_with("#!") {
            continue;
        }
        if is_magic_comment(line) {
            last_magic = Some(line_number);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        break;
    }
    let Some(line_number) = last_magic else {
        return;
    };
    let next = line_number + 1;
    if next > context.source.line_count() || context.source.line(next).trim().is_empty() {
        return;
    }
    let insertion = context.source.line_start(next);
    offenses.push(
        context
            .offense(
                "Add an empty line after magic comments.",
                insertion..insertion,
            )
            .corrected_by(Edit {
                start: insertion,
                end: insertion,
                replacement: "\n".to_owned(),
                safe: true,
            }),
    );
}

fn is_magic_comment(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("# frozen_string_literal:")
        || lower.starts_with("# encoding:")
        || lower.starts_with("# coding:")
        || (lower.starts_with("# -") && lower.contains("coding:"))
}
