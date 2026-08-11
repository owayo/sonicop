use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;
use unicode_width::UnicodeWidthStr;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(120);
    let allow_uri: bool = context.setting("AllowURI").unwrap_or(true);
    let allow_directives: bool = context.setting("AllowCopDirectives").unwrap_or(true);
    let allow_qualified_name: bool = context.setting("AllowQualifiedName").unwrap_or(true);
    let break_edits = line_break_edits(context, max);
    for line_number in 1..=context.source.line_count() {
        let raw = context.source.line(line_number);
        let line = raw.trim_end_matches(['\r', '\n']);
        let width = UnicodeWidthStr::width(line);
        let line_start = context.source.line_start(line_number);
        let line_range = line_start..line_start + line.len();
        if width <= max
            || context.in_heredoc(line_range)
            || (allow_uri && (line.contains("http://") || line.contains("https://")))
            || (allow_qualified_name && qualified_name_exempts_line(line, max))
            || (allow_directives && line.contains("rubocop:"))
        {
            continue;
        }
        let start = line_start
            + line
                .char_indices()
                .nth(max)
                .map_or(line.len(), |(index, _)| index);
        let offense = context.offense(
            format!("Line is too long. [{width}/{max}]"),
            start..line_start + line.len(),
        );
        offenses.push(match break_edits.get(&line_number) {
            Some(edit) => offense.corrected_by(edit.clone()),
            None => offense,
        });
    }
}

fn qualified_name_exempts_line(line: &str, max: usize) -> bool {
    static QUALIFIED_NAME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:[A-Z][A-Za-z0-9_]*::)+[A-Za-z_][A-Za-z0-9_]*\b").unwrap()
    });
    let Some(name) = QUALIFIED_NAME.find_iter(line).last() else {
        return false;
    };
    let start = line[..name.start()].chars().count();
    if start >= max {
        return false;
    }

    let suffix = &line[name.end()..];
    suffix.chars().all(|character| !character.is_whitespace())
        || (line.contains('{') && line.ends_with('}'))
}

fn line_break_edits(context: &RuleContext<'_>, max: usize) -> HashMap<usize, Edit> {
    let comments: HashSet<usize> = context
        .nodes_of("comment")
        .map(|node| node.start_position().row + 1)
        .collect();
    let mut edits = HashMap::new();

    // RuboCop gives a single-line block precedence over the call that owns it.
    // Breaking immediately after `{` / `do` is syntax preserving even when the
    // line has a trailing comment.
    for node in context
        .nodes_of_any(&["block", "do_block"])
        .filter(|node| node.start_position().row == node.end_position().row)
    {
        let start = node
            .child_by_field_name("parameters")
            .map_or_else(
                || node.start_byte() + if node.kind() == "block" { 1 } else { 2 },
                |parameters| parameters.end_byte(),
            )
            .min(node.end_byte());
        edits.entry(node.start_position().row + 1).or_insert(Edit {
            start,
            end: start,
            replacement: "\n".to_owned(),
            safe: true,
        });
    }

    for node in context
        .nodes_of_any(&["call", "array", "hash", "method", "singleton_method"])
        .filter(|node| breakable_collection_on_one_line(*node))
    {
        let line_number = node.start_position().row + 1;
        if edits.contains_key(&line_number) || comments.contains(&line_number) {
            continue;
        }

        let Some(mut elements) = breakable_elements(node, context) else {
            continue;
        };
        if elements.len() < 2 {
            continue;
        }

        if node.kind() == "call" && !call_parenthesized(node, context) {
            elements.remove(0);
        }
        let Some(element) = elements
            .iter()
            .position(|element| element.start_position().column > max)
            .map_or_else(
                || elements.last().copied(),
                |index| elements.get(index.saturating_sub(1)).copied(),
            )
        else {
            continue;
        };
        let start = element.start_byte();
        edits.insert(
            line_number,
            Edit {
                start,
                end: start,
                replacement: "\n".to_owned(),
                safe: true,
            },
        );
    }

    edits
}

fn breakable_collection_on_one_line(node: Node<'_>) -> bool {
    if node.kind() == "call" {
        return node
            .child_by_field_name("arguments")
            .is_some_and(|arguments| {
                node.start_position().row == arguments.start_position().row
                    && arguments.start_position().row == arguments.end_position().row
            });
    }
    node.start_position().row == node.end_position().row
}

fn breakable_elements<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Vec<Node<'tree>>> {
    let container = match node.kind() {
        "call" => node.child_by_field_name("arguments")?,
        "method" | "singleton_method" => node.child_by_field_name("parameters")?,
        "array" => node,
        "hash" if context.source.node_text(node).starts_with('{') => node,
        _ => return None,
    };
    let mut cursor = container.walk();
    Some(container.named_children(&mut cursor).collect())
}

fn call_parenthesized(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.child_by_field_name("arguments")
        .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
}
