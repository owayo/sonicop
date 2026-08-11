use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for line_number in 1..=context.source.line_count() {
        let range = context.source.line_range(line_number);
        let line = &text[range.clone()];
        let content_end = line.trim_end_matches(['\r', '\n']).len();
        let trimmed_end = line[..content_end].trim_end_matches([' ', '\t']).len();
        if trimmed_end < content_end {
            let start = range.start + trimmed_end;
            let end = range.start + content_end;
            offenses.push(
                context
                    .offense("Trailing whitespace detected.", start..end)
                    .corrected_by(Edit {
                        start,
                        end,
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
    }
}
