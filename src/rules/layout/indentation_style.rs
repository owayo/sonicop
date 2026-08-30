//! `Layout/IndentationStyle`.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let tabs = context
        .setting::<String>("EnforcedStyle")
        .as_deref()
        .unwrap_or("spaces")
        == "tabs";
    let width = usize::try_from(
        context
            .setting::<i64>("IndentationWidth")
            .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
            .unwrap_or(2),
    )
    .unwrap_or(2)
    .max(1);
    let message = match tabs {
        true => "Space detected in indentation.",
        false => "Tab detected in indentation.",
    };
    let unwanted = if tabs { b' ' } else { b'\t' };

    let mut literals: Option<Vec<Range<usize>>> = None;
    // `processed_source.lines` stops at `__END__`; the data section is not indented code.
    for line in 1..=crate::rules::support::last_code_line(context) {
        let start = context.source.line_start(line);
        let Some(end) = offending_indentation(context.source.line(line), unwanted) else {
            continue;
        };
        let range = start..(start + end);
        // Only worth building once a line looks wrong, which is what upstream defers it for.
        let literals = literals.get_or_insert_with(|| string_literals(context));
        if literals
            .iter()
            .any(|literal| range.start >= literal.start && range.end <= literal.end)
        {
            continue;
        }
        let source = &context.source.text()[range.clone()];
        let replacement = match source.contains('\t') {
            true => source.replace('\t', &" ".repeat(width)),
            false => "\t".repeat(source.chars().count() / width),
        };
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement,
            safe: true,
        }));
    }
}

/// `/\A\s*\t+/`, or `/\A\s* +/` when tabs are the style: the leading blanks up to and including the
/// last one written the wrong way.
fn offending_indentation(line: &str, unwanted: u8) -> Option<usize> {
    let blanks = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | 0x0b | 0x0c | b'\r'))
        .count();
    line.as_bytes()[..blanks]
        .iter()
        .rposition(|byte| *byte == unwanted)
        .map(|index| index + 1)
}

/// `string_literal_ranges`: where a line can begin inside a literal rather than in code. The body
/// of a heredoc counts, its terminator does not.
fn string_literals(context: &RuleContext<'_>) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = context
        .nodes_of("string")
        .map(|node| node.byte_range())
        .collect();
    for body in context.nodes_of("heredoc_body") {
        let _cursor = body.walk();
        // `loc.heredoc_body` stops at the start of the terminator's *line*, so the indentation a
        // squiggly heredoc's terminator was written with is code as far as this cop is concerned.
        let end = named_children_of(body, context)
            .into_iter()
            .find(|child| child.kind_str() == "heredoc_end")
            .map_or_else(
                || body.end_byte(),
                |child| {
                    context
                        .source
                        .line_start(context.source.line_column(child.start_byte()).0)
                },
            );
        ranges.push(body.start_byte()..end);
    }
    ranges
}
