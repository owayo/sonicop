use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // RuboCop takes every comment that precedes the first line of code and keeps the *last* magic
    // one among them. Stopping at the first magic comment reports the wrong line whenever a file
    // opens with several of them, which is common: an encoding line above a frozen string literal
    // line.
    let Some(line_number) = last_magic_comment_line(context) else {
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
                // `source_range` defaults to a length of one, so the offense covers the first
                // character of the line rather than collapsing to the insertion point.
                insertion..next_char_boundary(context, insertion),
            )
            .corrected_by(Edit {
                start: insertion,
                end: insertion,
                replacement: "\n".to_owned(),
                safe: true,
            }),
    );
}

fn last_magic_comment_line(context: &RuleContext<'_>) -> Option<usize> {
    let mut last = None;
    for line_number in 1..=context.source.line_count() {
        // This cop reads comment *tokens*, and a byte order mark is not part of the token that
        // follows it, so a magic comment on the first line still counts.
        let line = context
            .source
            .line(line_number)
            .trim_start_matches('\u{feff}')
            .trim();
        if !line.starts_with('#') {
            // The first line of code ends the run of comments RuboCop considers.
            if line.is_empty() {
                continue;
            }
            break;
        }
        if MagicComment::parse(line).any() {
            last = Some(line_number);
        }
    }
    last
}

fn next_char_boundary(context: &RuleContext<'_>, start: usize) -> usize {
    let text = context.source.text();
    let mut end = (start + 1).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    end
}
