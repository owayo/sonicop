//! `Layout/IndentationConsistency`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    alignment_corrections, begins_its_line, display_column, holds_block_comment, statement_groups,
    string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MESSAGE: &str = "Inconsistent indentation detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let indented_internal_methods = context
        .setting::<String>("EnforcedStyle")
        .as_deref()
        .unwrap_or("normal")
        == "indented_internal_methods";
    let text = context.source.text();

    // `@current_offenses` is the cop's whole list for the file: an item nested inside a span
    // already being realigned is reported without a correction of its own.
    let mut reported: Vec<Range<usize>> = Vec::new();
    for group in statement_groups(context) {
        if indented_internal_methods {
            // A modifier divides the body, and consistency is only asked for within a section.
            let mut section: Vec<Node<'_>> = Vec::new();
            for statement in &group.statements {
                if is_bare_access_modifier(text, *statement) {
                    check_alignment(context, &section, None, &mut reported, offenses);
                    section.clear();
                } else {
                    section.push(*statement);
                }
            }
            check_alignment(context, &section, None, &mut reported, offenses);
            continue;
        }
        let base = base_column_for_normal_style(context, &group.statements, group.parent_start);
        let items: Vec<Node<'_>> = group
            .statements
            .iter()
            .copied()
            .filter(|statement| !is_bare_access_modifier(text, *statement))
            .collect();
        check_alignment(context, &items, base, &mut reported, offenses);
    }
}

/// `base_column_for_normal_style`: a leading access modifier sets the column, unless it was
/// outdented to the enclosing body's own level.
fn base_column_for_normal_style(
    context: &RuleContext<'_>,
    statements: &[Node<'_>],
    parent_start: Option<usize>,
) -> Option<i64> {
    let first = *statements.first()?;
    if !is_bare_access_modifier(context.source.text(), first) {
        return None;
    }
    let modifier_indent = display_column(context, first.start_byte());
    let Some(parent_start) = parent_start else {
        return Some(modifier_indent);
    };
    (modifier_indent > display_column(context, parent_start)).then_some(modifier_indent)
}

/// A bare `public` / `protected` / `private` / `module_function`, which the grammar writes as a
/// plain identifier.
fn is_bare_access_modifier(text: &str, node: Node<'_>) -> bool {
    node.kind_str() == "identifier"
        && matches!(
            &text[node.byte_range()],
            "public" | "protected" | "private" | "module_function"
        )
}

fn check_alignment(
    context: &RuleContext<'_>,
    items: &[Node<'_>],
    base: Option<i64>,
    reported: &mut Vec<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    let Some(first) = items.first() else { return };
    let base = base.unwrap_or_else(|| display_column(context, first.start_byte()));
    let mut previous_line = 0usize;
    for item in items {
        let line = context.source.line_column(item.start_byte()).0;
        if line > previous_line && begins_its_line(context, item.start_byte()) {
            let delta = base - display_column(context, item.start_byte());
            if delta != 0 {
                report(context, item.byte_range(), delta, reported, offenses);
            }
        }
        previous_line = line;
    }
}

fn report(
    context: &RuleContext<'_>,
    item: Range<usize>,
    delta: i64,
    reported: &mut Vec<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    let nested = reported
        .iter()
        .any(|outer| item.start >= outer.start && item.end <= outer.end);
    let mut offense = context.offense(MESSAGE, item.clone());
    if !nested && !holds_block_comment(context, &item) {
        let taboo = string_interiors(context, &item);
        offense =
            offense.corrected_by_all(alignment_corrections(context, item.clone(), delta, &taboo));
    }
    reported.push(item);
    offenses.push(offense);
}
