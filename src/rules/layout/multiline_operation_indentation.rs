//! `Layout/MultilineOperationIndentation`.

use std::ops::Range;

use super::multiline_expression::{Mixin, UpKind, UpNode};
use super::support::{alignment_corrections, holds_block_comment};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mixin = Mixin::new(context, context.setting::<i64>("IndentationWidth"));
    let aligned = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "aligned");

    for ts in context.nodes_of_any(&["binary", "assignment"]) {
        let node = UpNode::plain(ts, context.ast_index());
        let checked = match node.kind(context) {
            // `on_and` / `on_or`.
            UpKind::And | UpKind::Or => {
                let (Some(lhs), Some(rhs)) = (
                    ts.field("left").map(|left| UpNode::of(left, context.ast_index())),
                    ts.field("right").map(|right| UpNode::of(right, context.ast_index())),
                ) else {
                    continue;
                };
                check_expression(&mixin, aligned, node, lhs, rhs.range(context))
            }
            // `on_send`: a binary operator call, and the `[]=` an index assignment builds.
            UpKind::Send | UpKind::Csend => {
                let Some(receiver) = node.receiver(context) else {
                    continue;
                };
                if node.method_name(context).as_deref() == Some("[]") {
                    continue;
                }
                // `relevant_node?`: a call written with a dot belongs to the other cop.
                if node.dot(context).is_some() {
                    continue;
                }
                let Some(rhs) = node.first_argument(context) else {
                    continue;
                };
                let lhs = mixin.left_hand_side(receiver);
                check_expression(&mixin, aligned, node, lhs, rhs.range(context))
            }
            _ => continue,
        };
        if let Some(offense) = checked {
            offenses.push(offense);
        }
    }
}

/// `MultilineExpressionIndentation#check` and the `offending_range` this cop defines.
fn check_expression<'tree>(
    mixin: &Mixin<'_, 'tree>,
    aligned: bool,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    rhs: Range<usize>,
) -> Option<Offense> {
    let context = mixin.context;
    if !mixin.begins_its_line(&rhs) {
        return None;
    }
    if mixin.not_for_this_cop(node) {
        return None;
    }
    let align = should_align(mixin, aligned, node, &rhs);
    let correct_column = if align {
        mixin.column(node.range(context).start)
    } else {
        mixin.indentation(lhs) + mixin.correct_indentation(node)
    };
    let delta = correct_column - mixin.column(rhs.start);
    if delta == 0 {
        return None;
    }

    let what = mixin.operation_description(node, &rhs);
    let message = if align {
        format!("Align the operands of {what} spanning multiple lines.")
    } else {
        let used = mixin.column(rhs.start) - mixin.indentation(lhs);
        format!(
            "Use {} (not {used}) spaces for indenting {what} spanning multiple lines.",
            mixin.correct_indentation(node)
        )
    };
    let mut offense = context.offense(message, rhs.clone());
    if !holds_block_comment(context, &rhs) {
        offense = offense.corrected_by_all(alignment_corrections(context, rhs, delta, &[]));
    }
    Some(offense)
}

/// `MultilineOperationIndentation#should_align?`.
fn should_align<'tree>(
    mixin: &Mixin<'_, 'tree>,
    aligned: bool,
    node: UpNode<'tree>,
    rhs: &Range<usize>,
) -> bool {
    let context = mixin.context;
    let assignment = mixin.part_of_assignment_rhs(node, Some(rhs));
    if let Some(assignment) = assignment {
        // `CheckAssignment.extract_rhs`: an assignment whose value starts on its own line puts the
        // operands under that value whatever the style says.
        if let Some(value) = mixin.assignment_rhs(assignment)
            && mixin.begins_its_line(&value.range(context))
        {
            return true;
        }
    }
    if !aligned {
        return false;
    }
    if mixin.keyword_ancestor(node).is_some() || assignment.is_some() {
        return true;
    }
    mixin
        .argument_in_method_call(node, false)
        .is_some_and(|call| !def_modifier(context, call))
}

/// `MethodDispatchNode#def_modifier?`: `private def foo`, where the call is a wrapper around a
/// definition rather than an ordinary argument list.
fn def_modifier(context: &RuleContext<'_>, node: UpNode<'_>) -> bool {
    let mut current = node;
    loop {
        if current.kind(context) != UpKind::Send || current.receiver(context).is_some() {
            return false;
        }
        let Some(argument) = current.first_argument(context) else {
            return false;
        };
        if matches!(argument.ts_kind(), "method" | "singleton_method") {
            return true;
        }
        current = argument;
    }
}
