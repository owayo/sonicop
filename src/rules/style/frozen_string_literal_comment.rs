use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.source.text().trim().is_empty() {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "always".to_owned());
    let lines: Vec<&str> = context.source.text().lines().take(4).collect();
    let existing = lines
        .iter()
        .position(|line| line.trim_start().starts_with("# frozen_string_literal:"));

    if style == "never" {
        if let Some(index) = existing {
            let start = context.source.line_start(index + 1);
            let end = context.source.line_range(index + 1).end;
            offenses.push(
                context
                    .offense("Remove the frozen string literal comment.", start..end)
                    .corrected_by(Edit {
                        start,
                        end,
                        replacement: String::new(),
                        safe: false,
                    }),
            );
        }
        return;
    }
    if existing.is_some() {
        return;
    }

    let first = context.source.line(1);
    let insertion = if first.starts_with("#!") {
        context.source.line_range(1).end
    } else {
        0
    };
    offenses.push(
        context
            .offense(
                "Missing frozen string literal comment.",
                insertion..insertion,
            )
            .corrected_by(Edit {
                start: insertion,
                end: insertion,
                replacement: "# frozen_string_literal: true\n\n".to_owned(),
                safe: false,
            }),
    );
}
