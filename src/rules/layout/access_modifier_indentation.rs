//! `Layout/AccessModifierIndentation`.

use tree_sitter::Node;

use super::support::{alignment_corrections, body_statements, character_column, end_keyword};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MODIFIERS: [&str; 4] = ["public", "protected", "private", "module_function"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let outdent = context.setting::<String>("EnforcedStyle").as_deref() == Some("outdent");
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let expected = if outdent { 0 } else { width };
    let style = if outdent { "Outdent" } else { "Indent" };

    for node in context.nodes_of_any(&["class", "module", "singleton_class", "block", "do_block"]) {
        // `node.body&.begin_type?`: a body of a single statement is that statement upstream, and
        // holds no run of members to line up.
        let Some(container) = body_container(node) else {
            continue;
        };
        let statements = body_statements(container);
        if statements.len() < 2 {
            continue;
        }
        let Some(end) = end_keyword(node).or_else(|| closing_brace(node)) else {
            continue;
        };
        // The line a block opens on is the line of the call it hangs off, not of its `do`.
        let owner = match node.kind() {
            "block" | "do_block" => node.parent().unwrap_or(node),
            _ => node,
        };
        let owner_line = context.source.line_column(owner.start_byte()).0;
        let end_column = character_column(context, end.start_byte());
        for modifier in statements {
            if !is_bare_access_modifier(context, modifier) {
                continue;
            }
            if context.source.line_column(modifier.start_byte()).0 == owner_line {
                continue;
            }
            let offset = character_column(context, modifier.start_byte()) - end_column;
            let delta = expected - offset;
            if delta == 0 {
                continue;
            }
            let message = format!(
                "{style} access modifiers like `{}`.",
                &context.source.text()[modifier.byte_range()]
            );
            offenses.push(
                context
                    .offense(message, modifier.byte_range())
                    .corrected_by_all(alignment_corrections(
                        context,
                        modifier.byte_range(),
                        delta,
                        &[],
                    )),
            );
        }
    }
}

/// `bare_access_modifier?`: `private` with nothing after it, which `private()` also is.
fn is_bare_access_modifier(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "identifier" => MODIFIERS.contains(&&context.source.text()[node.byte_range()]),
        "call" => {
            if node.child_by_field_name("receiver").is_some()
                || node.child_by_field_name("block").is_some()
            {
                return false;
            }
            let Some(method) = node.child_by_field_name("method") else {
                return false;
            };
            MODIFIERS.contains(&&context.source.text()[method.byte_range()])
                && node
                    .child_by_field_name("arguments")
                    .is_none_or(|arguments| arguments.named_child_count() == 0)
        }
        _ => false,
    }
}

fn body_container<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.named_children(&mut node.walk())
        .find(|child| matches!(child.kind(), "body_statement" | "block_body"))
}

fn closing_brace<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let last = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
    (last.kind() == "}").then_some(last)
}
