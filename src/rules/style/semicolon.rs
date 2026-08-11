use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::source::is_protected;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_as_expression_separator: bool = context
        .setting("AllowAsExpressionSeparator")
        .unwrap_or(false);
    let ranges = context.protected_ranges();
    let text = context.source.text();
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b';' || is_protected(index, ranges) {
            continue;
        }
        let rest = &text[index + 1..];
        let until_newline = rest.split_once('\n').map_or(rest, |(line, _)| line);
        if until_newline.trim_start().starts_with("end") {
            continue;
        }
        let only_comment =
            until_newline.trim().is_empty() || until_newline.trim_start().starts_with('#');
        let before_on_line = text[..index]
            .rsplit_once('\n')
            .map_or(&text[..index], |(_, line)| line);
        let adjacent_to_curly =
            until_newline.trim_start().starts_with('}') || before_on_line.trim_end().ends_with('{');
        if allow_as_expression_separator && !only_comment && !adjacent_to_curly {
            continue;
        }
        let offense = context.offense(
            "Do not use semicolons to terminate expressions.",
            index..index + 1,
        );
        // Only a semicolon with nothing but a comment after it can be dropped outright; removing
        // one that separates two expressions would join them into a single statement.
        offenses.push(if only_comment {
            offense.corrected_by(Edit {
                start: index,
                end: index + 1,
                replacement: String::new(),
                safe: true,
            })
        } else {
            offense
        });
    }
}
