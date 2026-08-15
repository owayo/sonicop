use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

use super::tokens::tokens;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if !context.source.text().contains('\\') {
        return;
    }
    let no_space = match context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "space".to_owned())
        .as_str()
    {
        "no_space" => true,
        "space" => false,
        _ => return,
    };
    let (message, replacement) = match no_space {
        true => ("Use zero spaces in front of backslash.", "\\"),
        false => ("Use one space in front of backslash.", " \\"),
    };
    // Everything after the last token -- the text below an `__END__`, above all -- is not lexed and
    // so is never looked at.
    let last_line = tokens(context)
        .last()
        .map_or(context.source.line_count(), |token| token.line);
    let ignored = ignored_ranges(context);
    for line in 1..=last_line {
        let range = context.source.line_range(line);
        let text = context.source.slice(range.clone());
        let Some(spacing) = offensive_spacing(text, no_space) else {
            continue;
        };
        // `source_range(buffer, line, line.length - spacing.length - 1, spacing.length)`, read in
        // characters: the span opens `spacing` back from the character after the backslash.
        let Some(column) = text
            .chars()
            .count()
            .checked_sub(spacing.chars().count() + 1)
        else {
            continue;
        };
        let start = range.start + byte_offset(text, column);
        let offense = start..start + spacing.len();
        if ignored.iter().any(|literal| contains(literal, &offense)) {
            continue;
        }
        offenses.push(
            context
                .offense(message, offense.clone())
                .corrected_by(Edit {
                    start: offense.start,
                    end: offense.end,
                    replacement: replacement.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `find_offensive_spacing`: the blanks in front of a trailing backslash that the style rejects,
/// together with the backslash itself.
///
/// The `no_space` style rejects any blank at all; the `space` style rejects none and two or more,
/// which is what its `((?<!\s)|\s{2,})\\$` says.
fn offensive_spacing(line: &str, no_space: bool) -> Option<&str> {
    let content = line.strip_suffix('\n').unwrap_or(line);
    let content = content.strip_suffix('\r').unwrap_or(content);
    let before = content.strip_suffix('\\')?;
    let blanks = before.len() - before.trim_end_matches([' ', '\t']).len();
    match no_space {
        true => (blanks > 0).then(|| &content[before.len() - blanks..]),
        false => match blanks {
            1 => None,
            0 => Some(&content[before.len()..]),
            _ => Some(&content[before.len() - blanks..]),
        },
    }
}

/// The byte offset the `column`-th character of `text` starts at.
fn byte_offset(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(offset, _)| offset)
}

/// `Parser::Source::Range#contains?`: contained, and strictly smaller on at least one side.
fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    inner.start >= outer.start
        && outer.end >= inner.end
        && (inner.start > outer.start || outer.end > inner.end)
}

/// `ignored_literal_ranges` plus the comments: the spans where a backslash is text rather than a
/// line continuation.
fn ignored_ranges(context: &RuleContext<'_>) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = context
        .nodes_of_any(&[
            // `str` / `dstr` with a `begin` delimiter: every quoted string.
            "string",
            // `array` that is a percent literal.
            "string_array",
            "symbol_array",
            // `literal.heredoc?`, whose ignored span is the body rather than the whole literal.
            "heredoc_body",
        ])
        .map(|node| node.byte_range())
        .collect();
    // A `str` inside a regexp or an `xstr` has no delimiter of its own, and is ignored because of
    // the literal it sits in. Upstream's parser gives the whole run of text between two
    // interpolations one `str` node, where the grammar splits it at every escape.
    for literal in context.nodes_of_any(&["regex", "subshell"]) {
        let mut run: Option<Range<usize>> = None;
        for child in named_children(literal) {
            match child.kind_str() == "interpolation" {
                true => ranges.extend(run.take()),
                false => {
                    run = Some(match run {
                        Some(range) => range.start..child.end_byte(),
                        None => child.byte_range(),
                    });
                }
            }
        }
        ranges.extend(run);
    }
    ranges.extend(context.comment_ranges().iter().cloned());
    ranges
}
