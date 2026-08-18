//! `Layout/EmptyLines`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `LINE_OFFSET`: two lines apart is one blank line between, which is allowed.
const LINE_OFFSET: usize = 2;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `return unless processed_source.raw_source.include?("\n\n\n")`.
    if !context.source.text().contains("\n\n\n") {
        return;
    }
    let lines = token_lines(context);
    if lines.is_empty() {
        return;
    }
    let mut previous = 1;
    for current in lines {
        if current > previous + LINE_OFFSET {
            for line in (previous + 1)..current {
                if !previous_and_current_lines_empty(context, line) {
                    continue;
                }
                let start = context.source.line_start(line);
                let range = start..start + 1;
                offenses.push(
                    context
                        .offense("Extra blank line detected.", range.clone())
                        .corrected_by(Edit {
                            start: range.start,
                            end: range.end,
                            replacement: String::new(),
                            safe: true,
                        }),
                );
            }
        }
        previous = current;
    }
}

/// `processed_source[line - 2].empty? && processed_source[line - 1].empty?`, where a line is the
/// source line with a single trailing newline chomped off -- a `\r\n` file therefore has no empty
/// line at all.
fn previous_and_current_lines_empty(context: &RuleContext<'_>, line: usize) -> bool {
    is_empty_line(context, line - 1) && is_empty_line(context, line)
}

fn is_empty_line(context: &RuleContext<'_>, line: usize) -> bool {
    let text = context.source.line(line);
    crate::rules::support::chomp(text).is_empty()
}

/// The lines `processed_source.tokens` covers.
///
/// A token is every terminal the lexer produces, comments included. The body of a string or a
/// heredoc is lexed one token per line, so a blank line inside a literal counts as occupied and a
/// leaf that spans lines claims all of them -- but a `=begin` block is a single token at its own
/// line, which leaves the blank lines inside it reportable. Everything after `__END__` is a data
/// section the lexer never reaches.
fn token_lines(context: &RuleContext<'_>) -> Vec<usize> {
    let mut lines: Vec<usize> = Vec::new();
    for node in context.nodes() {
        if node.child_count() > 0 || node.start_byte() == node.end_byte() {
            continue;
        }
        if matches!(node.kind_str(), "uninterpreted" | "__END__") {
            continue;
        }
        let first = node.start_position().row;
        let last = if node.kind_str() == "comment" {
            first
        } else {
            node.end_position().row
        };
        for row in first..=last {
            lines.push(row + 1);
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}
