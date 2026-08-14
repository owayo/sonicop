use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2)
        .max(0) as usize;
    let mut reported: Vec<std::ops::Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["if_modifier", "unless_modifier"]) {
        let (Some(body), Some(condition)) = (
            node.field("body"),
            node.field("condition"),
        ) else {
            continue;
        };
        if !is_multiline(body) {
            continue;
        }
        // `part_of_ignored_node?`: a modifier written inside one already reported is left for the
        // pass that runs on the rewritten text.
        if reported
            .iter()
            .any(|range| range.start <= node.start_byte() && node.end_byte() <= range.end)
        {
            continue;
        }
        reported.push(node.byte_range());
        let keyword = match node.kind_str() {
            "if_modifier" => "if",
            _ => "unless",
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Favor a normal {keyword}-statement over a modifier clause in a multiline statement."
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: normal_if(context, node, body, condition, keyword, width),
                    safe: true,
                }),
        );
    }
}

/// `multiline?`, which `BlockNode` overrides to compare the braces rather than the whole
/// expression: a chain broken over several lines ending in a one-line block is not multiline.
fn is_multiline(node: Node<'_>) -> bool {
    let span = match node.kind_str() {
        "call" => node.field("block"),
        "lambda" => node.field("body"),
        _ => None,
    }
    .unwrap_or(node);
    span.start_position().row != span.end_position().row
}

/// `to_normal_if`: the condition on its own line, the body indented one step further, and an `end`
/// under the column the modifier started at.
fn normal_if(
    context: &RuleContext<'_>,
    node: Node<'_>,
    body: Node<'_>,
    condition: Node<'_>,
    keyword: &str,
    width: usize,
) -> String {
    let (_, column) = context.source.line_column(node.start_byte());
    let offset = " ".repeat(column - 1);
    let indentation = format!("{offset}{}", " ".repeat(width));
    let body_source = format!("{offset}{}", context.source.node_text(body));
    let indented: String = body_source
        .split_inclusive('\n')
        .map(|line| match line == "\n" {
            true => line.to_owned(),
            false => replace_leading_blanks(line, offset.len(), &indentation),
        })
        .collect();
    format!(
        "{keyword} {}\n{indented}\n{offset}end",
        context.source.node_text(condition)
    )
}

/// `line.sub(/^\s{n}/, indentation)`: the first `n` whitespace characters of the line, and only
/// those, stand in for the indentation the line is given.
fn replace_leading_blanks(line: &str, count: usize, indentation: &str) -> String {
    let matched: usize = line
        .chars()
        .take(count)
        .take_while(|character| character.is_whitespace())
        .map(char::len_utf8)
        .sum();
    if line[..matched].chars().count() < count {
        return line.to_owned();
    }
    format!("{indentation}{}", &line[matched..])
}
