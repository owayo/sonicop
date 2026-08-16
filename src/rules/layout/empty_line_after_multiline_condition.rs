use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;
use crate::rules::support;

const MSG: &str = "Use empty line after multiline condition.";

/// The node kinds the grammar adds for statement lists, whose siblings are the statements upstream's
/// `begin` holds.
const CONTAINERS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "do",
    "block_body",
    "parenthesized_statements",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&[
        "if",
        "unless",
        "elsif",
        "if_modifier",
        "unless_modifier",
        "while",
        "until",
        "while_modifier",
        "until_modifier",
        "case",
        "rescue",
    ]) {
        match node.kind_str() {
            "case" => offenses.extend(when_branches(node, context)),
            "rescue" => offenses.extend(rescue_branch(node, context)),
            kind => {
                // `on_if` reads a modifier form only where something follows it, and so does
                // `on_while_post` -- which is what `begin ... end while cond` builds.
                let needs_sibling = matches!(kind, "if_modifier" | "unless_modifier")
                    || is_post_condition_loop(node);
                if needs_sibling && next_statement(node, context).is_none() {
                    continue;
                }
                let Some(condition) = node.field("condition") else {
                    continue;
                };
                offenses.extend(check_condition(condition, condition, context));
            }
        }
    }
}

/// `check_condition`: a condition spread over several lines wants a blank line under it.
fn check_condition(
    reported: Node<'_>,
    condition: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    if !is_multiline(condition, context) {
        return None;
    }
    let last_line = line(condition.end_byte(), context);
    if is_blank(last_line + 1, context) {
        return None;
    }
    Some(offense(reported, condition, context))
}

/// `node.multiline?`, which `BlockNode` answers from its own delimiters rather than from the span of
/// the whole expression.
///
/// `%w(\n  A\n).any? { |k| k }` is one line by that reading even though it was written over three,
/// and a condition ending in such a call needs no blank line under it.
fn is_multiline(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let measured = match block_delimiters(node) {
        Some(block) => block,
        None => node.byte_range(),
    };
    line(measured.start, context) != line(measured.end, context)
}

/// The `{` .. `}` or `do` .. `end` a block was written with, when the node is one.
fn block_delimiters(node: Node<'_>) -> Option<Range<usize>> {
    let delimited = match node.kind_str() {
        "call" => node.field("block")?,
        "lambda" => node.field("body")?,
        _ => return None,
    };
    Some(delimited.byte_range())
}

/// `on_case`: each `when` whose own list of conditions spans more than one line.
fn when_branches(node: Node<'_>, context: &RuleContext<'_>) -> Vec<Offense> {
    let mut offenses = Vec::new();
    for branch in named_children(node) {
        if branch.kind_str() != "when" {
            continue;
        }
        let conditions: Vec<Node<'_>> = named_children(branch)
            .into_iter()
            .filter(|child| child.kind_str() == "pattern")
            .collect();
        let (Some(first), Some(last)) = (conditions.first(), conditions.last()) else {
            continue;
        };
        // `multiline_when_condition?` measures the whole list rather than one condition.
        if line(first.start_byte(), context) == line(last.end_byte(), context) {
            continue;
        }
        if is_blank(line(last.end_byte(), context) + 1, context) {
            continue;
        }
        offenses.push(offense(branch, *last, context));
    }
    offenses
}

/// `on_rescue`: each `resbody` that names more than one exception across more than one line.
fn rescue_branch(node: Node<'_>, context: &RuleContext<'_>) -> Option<Offense> {
    let exceptions = named_children(node.field("exceptions")?);
    // `multiline_rescue_exceptions?`: one exception is never enough.
    if exceptions.len() <= 1 {
        return None;
    }
    let (first, last) = (exceptions.first()?, exceptions.last()?);
    if line(first.start_byte(), context) == line(last.end_byte(), context) {
        return None;
    }
    if is_blank(line(last.end_byte(), context) + 1, context) {
        return None;
    }
    Some(offense(node, *last, context))
}

/// The offense, whose correction writes a blank line under the last line of `anchor`.
fn offense(reported: Node<'_>, anchor: Node<'_>, context: &RuleContext<'_>) -> Offense {
    let lines = support::whole_lines_without_terminator(anchor.byte_range(), context);
    context
        .offense(MSG, reported.byte_range())
        .corrections_anchored_at(lines.clone())
        .corrected_by(Edit {
            start: lines.end,
            end: lines.end,
            replacement: "\n".to_owned(),
            safe: true,
        })
}

/// `while_post` / `until_post`: a modifier loop whose body is a `begin ... end`, which upstream reads
/// as a loop that runs its body first.
fn is_post_condition_loop(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "while_modifier" | "until_modifier")
        && node
            .field("body")
            .is_some_and(|body| body.kind_str() == "begin")
}

/// `node.right_sibling`: the statement written after this one.
fn next_statement<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let parent = node.parent_of(context)?;
    if !CONTAINERS.contains(&parent.kind_str()) {
        return None;
    }
    let siblings: Vec<Node<'_>> = named_children(parent)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    let position = siblings
        .iter()
        .position(|sibling| sibling.id() == node.id())?;
    siblings.get(position + 1).copied()
}

/// `next_line_empty?`, which reads a line past the end of the file as empty.
fn is_blank(line: usize, context: &RuleContext<'_>) -> bool {
    line > context.source.line_count() || context.source.line(line).trim().is_empty()
}

fn line(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
