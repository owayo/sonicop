use super::support::heredocs;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "uppercase".to_owned());
    let safe: bool = context.setting("Safe").unwrap_or(true);
    let convert = |text: &str| {
        if style == "lowercase" {
            text.to_lowercase()
        } else {
            text.to_uppercase()
        }
    };

    for heredoc in heredocs(context) {
        let delimiter = heredoc.delimiter(context.source);
        if delimiter == convert(delimiter) {
            continue;
        }
        // Upstream rewrites the opening and the terminator as two separate replacements. One edit
        // has to stand for both, so it spans from the `<<` to the end of the terminator and hands
        // back the text in between unchanged.
        let span = heredoc.opening.start..heredoc.heredoc_end.end;
        let opening = context.source.slice(heredoc.opening.clone());
        let middle = context
            .source
            .slice(heredoc.opening.end..heredoc.heredoc_end.start);
        // The replacement for the terminator is the delimiter alone, while the range it replaces
        // starts at the beginning of the line: correcting an indented terminator pulls it back to
        // column zero, exactly as upstream does.
        let replacement = format!("{}{middle}{}", convert(opening), convert(delimiter));
        offenses.push(
            context
                .offense(
                    format!("Use {style} heredoc delimiters."),
                    heredoc.heredoc_end,
                )
                .corrected_by(Edit {
                    start: span.start,
                    end: span.end,
                    replacement,
                    safe,
                }),
        );
    }
}
