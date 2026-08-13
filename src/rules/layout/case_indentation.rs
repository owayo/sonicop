//! `Layout/CaseIndentation`.

use tree_sitter::Node;

use super::support::{character_column, end_keyword};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let align_with_end = context.setting::<String>("EnforcedStyle").as_deref() == Some("end");
    let one_step = context.setting::<bool>("IndentOneStep").unwrap_or(false);
    let width: i64 = match one_step {
        true => context
            .setting::<i64>("IndentationWidth")
            .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
            .unwrap_or(2),
        false => 0,
    };
    let style = if align_with_end { "end" } else { "case" };
    let depth = if one_step {
        "one step more than"
    } else {
        "as deep as"
    };

    for node in context.nodes_of_any(&["case", "case_match"]) {
        if node.start_position().row == node.end_position().row {
            continue;
        }
        if align_with_end && end_and_last_conditional_same_line(context, node) {
            continue;
        }
        let Some(base) = base_column(context, node, align_with_end) else {
            continue;
        };
        let branch_kind = match node.kind_str() {
            "case" => "when",
            _ => "in_clause",
        };
        let branch_type = match node.kind_str() {
            "case" => "when",
            _ => "in",
        };
        for branch in node.named_children(&mut node.walk()) {
            if branch.kind_str() != branch_kind {
                continue;
            }
            let Some(keyword) = branch.child(0) else {
                continue;
            };
            let column = character_column(context, keyword.start_byte());
            if column == base + width {
                continue;
            }
            let message = format!("Indent `{branch_type}` {depth} `{style}`.");
            // `whitespace_range`: everything on the branch's line before the keyword. A keyword
            // that shares its line with code has no indentation to rewrite, and upstream then
            // hands its corrector nothing at all.
            let line = context.source.line_column(keyword.start_byte()).0;
            let whitespace = context.source.line_start(line)..keyword.start_byte();
            let mut offense = context.offense(message, keyword.byte_range());
            if context.source.text()[whitespace.clone()].trim().is_empty() {
                let target = usize::try_from(base + width).unwrap_or(0);
                offense = offense.corrected_by(Edit {
                    start: whitespace.start,
                    end: whitespace.end,
                    replacement: " ".repeat(target),
                    safe: true,
                });
            }
            offenses.push(offense);
        }
    }
}

/// `base_column`: the `case` keyword, or the `end` that closes it.
fn base_column(context: &RuleContext<'_>, node: Node<'_>, align_with_end: bool) -> Option<i64> {
    let anchor = match align_with_end {
        true => end_keyword(node)?,
        false => node.child(0)?,
    };
    Some(character_column(context, anchor.start_byte()))
}

/// `end_and_last_conditional_same_line?`: `end` written on the line of the last branch's `then`,
/// or of the `else`, has nothing left to line the branches up against.
fn end_and_last_conditional_same_line(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(end) = end_keyword(node) else {
        return false;
    };
    let end_line = context.source.line_column(end.start_byte()).0;
    let branches: Vec<Node<'_>> = node.named_children(&mut node.walk()).collect();
    let Some(last) = branches.last() else {
        return false;
    };
    let anchor = match last.kind_str() {
        "else" => last.child(0),
        // `node.child_nodes.last.loc.begin`: the `then` of the last branch.
        _ => last
            .children(&mut last.walk())
            .find(|child| child.kind_str() == "then")
            .and_then(|then| then.child(0))
            .filter(|keyword| keyword.kind_str() == "then"),
    };
    anchor.is_some_and(|anchor| context.source.line_column(anchor.start_byte()).0 == end_line)
}
