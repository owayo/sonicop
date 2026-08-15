use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::directives::comment_disables_cop;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::top_level_constant;

use super::statements::body_children;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_empty_lambdas: bool = context.setting("AllowEmptyLambdas").unwrap_or(true);
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    for block in context.nodes_of_any(&["block", "do_block"]) {
        if !is_empty(block) {
            continue;
        }
        // Upstream's `block` node is the call and the braces together; the grammar hangs the
        // braces off the call instead, so the span the cop reports is the parent's.
        let Some(node) = block.parent_of(context) else {
            continue;
        };
        if allow_empty_lambdas && lambda_or_proc(node, context) {
            continue;
        }
        if allow_comments && allow_comment(node, context) {
            continue;
        }
        offenses.push(context.offense("Empty block detected.", node.byte_range()));
    }
}

fn is_empty(block: Node<'_>) -> bool {
    block
        .field("body")
        .is_none_or(|body| body_children(body).is_empty())
}

/// `lambda_or_proc?`: `lambda`, `proc`, `Proc.new` and the stabby form, which the grammar gives a
/// node of its own rather than a call.
fn lambda_or_proc(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "lambda" {
        return true;
    }
    let Some(method) = node.field("method") else {
        return false;
    };
    let name = context.source.node_text(method);
    if matches!(name, "lambda" | "proc") {
        return node.field("receiver").is_none();
    }
    name == "new"
        && node
            .field("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "Proc", context))
}

/// `allow_comment?`: a comment anywhere in the block excuses it, unless the one on the line the
/// block opens on is the directive that turns this cop off.
fn allow_comment(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let range = node.byte_range();
    if !crate::rules::support::contains_comment(context, range.clone()) {
        return false;
    }
    let (line, _) = context.source.line_column(range.start);
    let Some(comment) = context.comment_ranges().iter().find(|comment| {
        let (comment_line, _) = context.source.line_column(comment.start);
        comment_line == line
    }) else {
        return true;
    };
    !comment_disables_cop(context.source.slice(comment.clone()), "Lint/EmptyBlock")
}
