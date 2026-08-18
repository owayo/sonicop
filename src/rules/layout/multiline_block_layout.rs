//! `Layout/MultilineBlockLayout`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::{body_statements, character_column, final_pos, parser_node_start};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Block body expression is on the same line as the block start.";
const ARG_MSG: &str = "Block argument expression is not on the same line as the block start.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    let maximum = max_line_length(context);
    for node in context.nodes_of_any(&["block", "do_block"]) {
        let (Some(open), Some(close)) = (block_open(node), block_close(node)) else {
            continue;
        };
        // `BlockNode#single_line?` compares the two delimiters rather than the whole expression.
        if open.start_position().row == close.start_position().row {
            continue;
        }
        let arguments = arguments(node);
        let body = body_range(node);

        let on_beginning_line = arguments
            .is_none_or(|arguments| open.start_position().row == arguments.end_position().row);

        // `autocorrect` is the same for both checks and is decided on its own terms: the argument
        // list moves up, and the body follows only if it shared a line with whatever preceded it.
        let mut edits = Vec::new();
        let mut expression_before_body = open.start_byte();
        if let Some(arguments) = arguments.filter(|_| !on_beginning_line) {
            let end = final_pos(text, arguments.end_byte(), true, false, false, false);
            edits.push(Edit {
                start: open.end_byte(),
                end,
                replacement: format!(" |{}|", argument_string(context, arguments)),
                safe: true,
            });
            expression_before_body = arguments.end_byte();
        }
        if let Some(body) = &body {
            if context.source.line_column(expression_before_body).0
                == context.source.line_column(body.start).0
            {
                let column = character_column(context, parser_node_start(node));
                edits.push(Edit {
                    start: body.start,
                    end: body.start,
                    replacement: format!(
                        "\n  {}",
                        " ".repeat(usize::try_from(column).unwrap_or(0))
                    ),
                    safe: true,
                });
            }
        }

        // Only one of the two can report: an argument list running past the delimiter's line has
        // pushed the body off that line as well.
        if let Some(arguments) = arguments.filter(|_| !on_beginning_line) {
            if !line_break_necessary(context, node, arguments, maximum) {
                offenses.push(
                    context
                        .offense(ARG_MSG, arguments.byte_range())
                        .corrected_by_all(edits.clone()),
                );
            }
        }
        if let Some(body) = body {
            if open.start_position().row == context.source.line_column(body.start).0 - 1 {
                offenses.push(context.offense(MSG, body).corrected_by_all(edits));
            }
        }
    }
}

fn max_line_length(context: &RuleContext<'_>) -> Option<i64> {
    if context.setting_of::<bool>("Layout/LineLength", "Enabled") == Some(false) {
        return None;
    }
    Some(
        context
            .setting_of::<i64>("Layout/LineLength", "Max")
            .unwrap_or(120),
    )
}

/// `line_break_necessary_in_args?`: the arguments would not fit on the delimiter's line anyway.
fn line_break_necessary(
    context: &RuleContext<'_>,
    node: Node<'_>,
    arguments: Node<'_>,
    maximum: Option<i64>,
) -> bool {
    let Some(maximum) = maximum else {
        return false;
    };
    let start = parser_node_start(node);
    let source = &context.source.text()[start..node.end_byte()];
    let first_line = source.split_inclusive('\n').next().unwrap_or(source);
    let pipes = if first_line.ends_with("|\n") { 1 } else { 3 };
    let needed = character_column(context, start)
        + pipes
        + crate::rules::support::chomp(first_line).chars().count() as i64
        + argument_string(context, arguments).chars().count() as i64;
    needed > maximum
}

/// `block_arg_string`: the parameters rewritten onto one line.
fn argument_string(context: &RuleContext<'_>, arguments: Node<'_>) -> String {
    let parts = parameter_nodes(arguments);
    let mut joined = parts
        .iter()
        .map(|part| match part.kind_str() {
            "destructured_parameter" => format!("({})", argument_string(context, *part)),
            _ => context.source.node_text(*part).to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if positional_count(arguments) == 1 && context.source.node_text(arguments).contains(',') {
        joined.push(',');
    }
    joined
}

fn parameter_nodes<'tree>(arguments: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = arguments.walk();
    arguments
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect()
}

/// `args.each_descendant(:arg).size`: how many plain positional parameters the list holds, counting
/// the ones inside a destructured parameter.
fn positional_count(arguments: Node<'_>) -> usize {
    parameter_nodes(arguments)
        .into_iter()
        .map(|part| match part.kind_str() {
            "identifier" => 1,
            "destructured_parameter" => positional_count(part),
            _ => 0,
        })
        .sum()
}

fn block_open<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| matches!(child.kind_str(), "{" | "do"))
}

fn block_close<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| matches!(child.kind_str(), "}" | "end"))
        .last()
}

fn arguments<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("parameters")
        .filter(|parameters| parameters.kind_str() == "block_parameters")
        .filter(|parameters| !parameter_nodes(*parameters).is_empty())
}

/// `node.body.source_range`: from the first statement of the block to the last, comments excluded.
fn body_range(node: Node<'_>) -> Option<Range<usize>> {
    let body = node.field("body")?;
    let statements = body_statements(body);
    let (first, last) = (statements.first()?, statements.last()?);
    Some(first.start_byte()..last.end_byte())
}
