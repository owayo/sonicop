//! `Layout/LineEndStringConcatenationIndentation`: how the parts of a backslash-joined string line
//! up.

use tree_sitter::Node;

use super::support::{alignment_corrections, character_column, line_indentation, string_interiors};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_ALIGN: &str = "Align parts of a string concatenated with backslash.";
const MSG_INDENT: &str = "Indent the first part of a string concatenated with backslash.";

/// `PARENT_TYPES_FOR_INDENTED`: the places a concatenation stands as a statement of its own, where
/// the parts are indented rather than aligned.
const INDENTED_PARENTS: &[&str] = &[
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    "do",
    "begin",
    "block",
    "do_block",
    "method",
    "singleton_method",
    "if",
    "unless",
    "elsif",
    "conditional",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let indented_style = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "indented");
    let width = indentation_width(context);
    for node in context.nodes_of("chained_string") {
        let parts = named_children(node);
        if !concatenated_with_backslash(node, &parts) || parts.is_empty() {
            continue;
        }
        if !indented_style && !always_indented(node, context) {
            check_aligned(context, &parts, 1, offenses);
        } else {
            check_indented(context, node, &parts, width, offenses);
            check_aligned(context, &parts, 2, offenses);
        }
    }
}

/// `strings_concatenated_with_backslash?`.
fn concatenated_with_backslash(node: Node<'_>, parts: &[Node<'_>]) -> bool {
    node.start_position().row != node.end_position().row
        && parts
            .iter()
            .all(|part| matches!(part.kind_str(), "string" | "chained_string"))
        && parts
            .iter()
            .all(|part| part.start_position().row == part.end_position().row)
}

/// `always_indented?`: the concatenation is a statement rather than a value handed to something.
fn always_indented(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match context.parent(node) {
        None => true,
        Some(parent) => INDENTED_PARENTS.contains(&parent.kind_str()),
    }
}

/// `check_aligned`: every part after the first lines up with the one before it.
fn check_aligned(
    context: &RuleContext<'_>,
    parts: &[Node<'_>],
    start: usize,
    offenses: &mut Vec<Offense>,
) {
    if start == 0 || parts.len() < start + 1 {
        return;
    }
    let mut base = character_column(context, parts[start - 1].start_byte());
    for part in &parts[start..] {
        let column = character_column(context, part.start_byte());
        let delta = base - column;
        if delta != 0 {
            report(context, *part, MSG_ALIGN, delta, offenses);
        }
        // The next comparison runs against where the part actually is, not where it was asked to
        // be, so one misplaced part does not make every part after it an offense.
        base = column;
    }
}

/// `check_indented`: the second part sits one level in from where the first part's line begins.
fn check_indented(
    context: &RuleContext<'_>,
    node: Node<'_>,
    parts: &[Node<'_>],
    width: i64,
    offenses: &mut Vec<Offense>,
) {
    if parts.len() < 2 {
        return;
    }
    let delta = base_column(context, node, parts[0]) + width
        - character_column(context, parts[1].start_byte());
    if delta != 0 {
        report(context, parts[1], MSG_INDENT, delta, offenses);
    }
}

/// `base_column`: the column a pair's key opens at, or where the part's own line begins.
fn base_column(context: &RuleContext<'_>, node: Node<'_>, part: Node<'_>) -> i64 {
    match context.parent(node) {
        Some(grandparent) if grandparent.kind_str() == "pair" => {
            character_column(context, grandparent.start_byte())
        }
        _ => line_indentation(context, part.start_byte()),
    }
}

fn report(
    context: &RuleContext<'_>,
    part: Node<'_>,
    message: &str,
    delta: i64,
    offenses: &mut Vec<Offense>,
) {
    let expr = part.byte_range();
    let taboo = string_interiors(context, &expr);
    offenses.push(
        context
            .offense(message, expr.clone())
            .corrected_by_all(alignment_corrections(context, expr, delta, &taboo)),
    );
}

/// `configured_indentation_width`.
fn indentation_width(context: &RuleContext<'_>) -> i64 {
    context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2)
}

fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind_str() != "comment")
        .collect()
}
