use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

const MSG: &str = "Avoid multi-line method signatures.";

/// `"def".len()`.
const DEF_LENGTH: usize = 3;

/// `max_line_length`'s fallback when `Layout/LineLength` names no `Max`.
const DEFAULT_MAX_LINE_LENGTH: usize = 120;

/// A `def` whose parameter list runs past the line the keyword is on.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max_line_length = if context.cop_enabled("Layout/LineLength") {
        Some(
            context
                .setting_of::<usize>("Layout/LineLength", "Max")
                .unwrap_or(DEFAULT_MAX_LINE_LENGTH),
        )
    } else {
        None
    };
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        let written = super::nodes::children(parameters);
        let (Some(first), Some(last)) = (written.first(), written.last()) else {
            continue;
        };
        let opening_line = context.source.line_column(node.start_byte()).0;
        let closing_line = context.source.line_column(parameters.end_byte()).0;
        if opening_line == closing_line {
            continue;
        }
        if exceeds_max_line_length(node, parameters, max_line_length, context) {
            continue;
        }
        // `node.arguments.loc.begin`: only a parenthesised list can be folded onto one line.
        if !context.source.node_text(parameters).starts_with('(') {
            continue;
        }
        let text = context.source.text();
        let mut joined = written
            .iter()
            .map(|parameter| context.source.node_text(*parameter))
            .collect::<Vec<&str>>()
            .join(", ");
        let mut edits = Vec::new();
        // `last_line_source_of_arguments.start_with?(')')`: a closing paren on a line of its own
        // travels with the arguments.
        let closing_line_text = context.source.line(closing_line).trim();
        if closing_line_text.starts_with(')') {
            joined.push_str(closing_line_text);
            let closing = parameters.end_byte() - 1..parameters.end_byte();
            edits.push(remove(support::whole_lines(closing, context)));
        }
        // `range_with_surrounding_space(arguments_range(node), side: :left)`.
        let arguments_range =
            support::final_pos(text, first.start_byte(), false, true, false)..last.end_byte();
        // A list that starts on a later line leaves the name alone on the first one.
        if context.source.line_column(arguments_range.start).0 != opening_line {
            let prefix = node.start_byte() + DEF_LENGTH..parameters.start_byte();
            edits.push(Edit {
                start: prefix.start,
                end: prefix.end,
                replacement: format!(" {}", text[prefix.clone()].trim()),
                safe: true,
            });
        }
        edits.push(remove(arguments_range));
        // `corrector.insert_after(begin_of_arguments, joined_arguments)`.
        edits.push(Edit {
            start: parameters.start_byte() + 1,
            end: parameters.start_byte() + 1,
            replacement: joined,
            safe: true,
        });
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `correction_exceeds_max_line_length?`.
fn exceeds_max_line_length(
    node: Node<'_>,
    parameters: Node<'_>,
    max_line_length: Option<usize>,
    context: &RuleContext<'_>,
) -> bool {
    let Some(max) = max_line_length else {
        return false;
    };
    let line = context.source.line_column(node.start_byte()).0;
    let indentation = context
        .source
        .line(line)
        .chars()
        .take_while(|character| character.is_whitespace())
        .count();
    // `definition_width`: the signature with every run of whitespace squeezed to one space.
    let signature = &context.source.text()[node.start_byte()..parameters.end_byte()];
    let width = squeeze(signature).chars().count();
    indentation + width > max
}

/// `String#gsub(/\s+/, ' ')`.
fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            in_space = false;
            out.push(character);
        }
    }
    out
}

fn remove(range: std::ops::Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}
