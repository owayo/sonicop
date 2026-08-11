use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;

const MSG_MISSING: &str = "Missing frozen string literal comment.";
const MSG_MISSING_TRUE: &str = "Missing magic comment `# frozen_string_literal: true`.";
const MSG_UNNECESSARY: &str = "Unnecessary frozen string literal comment.";
const MSG_DISABLED: &str = "Frozen string literal comment must be set to `true`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.source.text().trim().is_empty() {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "always".to_owned());

    match style.as_str() {
        "never" => {
            if let Some(comment) = specified_comment(context) {
                offenses.push(
                    context
                        .offense(MSG_UNNECESSARY, comment.clone())
                        .corrected_by(remove_comment(context, comment)),
                );
            }
        }
        "always_true" => {
            // The setting has to be named *and* set to `true`; naming it and disabling it is its own
            // offense, reported on the comment rather than at the head of the file.
            if !leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).frozen_string_literal_specified())
            {
                offenses.push(missing_offense(context, MSG_MISSING_TRUE));
            } else if !leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).frozen_string_literal_enabled())
                && let Some(comment) = specified_comment(context)
            {
                offenses.push(
                    context
                        .offense(MSG_DISABLED, comment.clone())
                        .corrected_by(Edit {
                            start: comment.start,
                            end: comment.end,
                            replacement: "# frozen_string_literal: true".to_owned(),
                            safe: false,
                        }),
                );
            }
        }
        // RuboCop only counts a comment that actually reaches Ruby: the value has to be `true` or
        // `false`, so `# frozen_string_literal: yes` leaves the file still missing one.
        _ => {
            if !leading_comments(context)
                .any(|(_, line)| MagicComment::parse(line).valid_literal_value())
            {
                offenses.push(missing_offense(context, MSG_MISSING));
            }
        }
    }
}

fn missing_offense(context: &RuleContext<'_>, message: &str) -> Offense {
    // The offense is reported at the head of the file, as RuboCop does: the comment is missing from
    // the file, not from a particular line. Only the insertion point moves, since the comment goes
    // after a shebang and an encoding comment rather than displacing them. `source_range` defaults
    // to a length of one, so the reported range covers the first character instead of collapsing to
    // a caret.
    let edit = match last_special_comment_line(context) {
        // Inserted at the end of that line's text, so the newline already there ends the new line.
        Some(line_number) => {
            let end = line_content_end(context, line_number);
            Edit {
                start: end,
                end,
                replacement: "\n# frozen_string_literal: true".to_owned(),
                safe: false,
            }
        }
        None => Edit {
            start: 0,
            end: 0,
            replacement: "# frozen_string_literal: true\n".to_owned(),
            safe: false,
        },
    };
    // `source_range` counts a length of one in characters, so the range has to span the whole first
    // character -- a byte order mark is three bytes and still just one of them.
    let first_character = context
        .source
        .text()
        .chars()
        .next()
        .map_or(1, char::len_utf8);
    context
        .offense(message, 0..first_character)
        .corrected_by(edit)
}

/// The shebang and the encoding comment the new comment has to follow. RuboCop only recognizes a
/// UTF-8 encoding comment here, since that is the only one its `Style/Encoding` pattern matches.
fn last_special_comment_line(context: &RuleContext<'_>) -> Option<usize> {
    let first = (1..=context.source.line_count())
        .find(|line_number| !context.source.line(*line_number).trim().is_empty())?;

    let mut special = None;
    let mut candidate = first;
    if context.source.line(first).trim_start().starts_with("#!") {
        special = Some(first);
        candidate = first + 1;
    }
    if candidate <= context.source.line_count()
        && is_utf8_encoding_comment(context.source.line(candidate))
    {
        special = Some(candidate);
    }
    special
}

fn is_utf8_encoding_comment(line: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"#.*coding\s?[:=]\s?(?:UTF|utf)-8")
            .expect("encoding comment pattern must compile")
    });
    PATTERN.is_match(line)
}

/// The end of a line's text, before its newline, which is where RuboCop's `line_range` stops.
fn line_content_end(context: &RuleContext<'_>, line_number: usize) -> usize {
    let range = context.source.line_range(line_number);
    range.start
        + context
            .source
            .line(line_number)
            .trim_end_matches(['\n', '\r'])
            .len()
}

/// Removes the comment along with the line it sits on, so no blank line is left behind.
fn remove_comment(context: &RuleContext<'_>, comment: Range<usize>) -> Edit {
    let line_number = context.source.line_column(comment.start).0;
    Edit {
        start: context.source.line_start(line_number),
        end: context.source.line_range(line_number).end,
        replacement: String::new(),
        safe: false,
    }
}

/// The comment lines above the first line that holds code, paired with their line number. Blank
/// lines between comments belong to the span, so this is a line range rather than a run of comments.
fn leading_comments<'a>(
    context: &'a RuleContext<'a>,
) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    let first_code = (1..=context.source.line_count()).find(|line_number| {
        let line = context.source.line(*line_number).trim();
        !line.is_empty() && !line.starts_with('#')
    });
    let end = first_code.unwrap_or(context.source.line_count() + 1);
    (1..end).map(|line_number| (line_number, context.source.line(line_number)))
}

/// The first comment anywhere in the file that names the setting, which is what the `never` style
/// removes and the `always_true` style rewrites.
fn specified_comment(context: &RuleContext<'_>) -> Option<Range<usize>> {
    (1..=context.source.line_count()).find_map(|line_number| {
        let line = context.source.line(line_number);
        let trimmed = line.trim();
        if !trimmed.starts_with('#')
            || !MagicComment::parse(trimmed).frozen_string_literal_specified()
        {
            return None;
        }
        let start = context.source.line_start(line_number) + (line.len() - line.trim_start().len());
        Some(start..start + trimmed.len())
    })
}
