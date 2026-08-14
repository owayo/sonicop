use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;
use crate::rules::support;

/// The kinds tree-sitter writes a block as. Upstream's `block` node covers the call and the block
/// together, which is what a `lambda` node and a `call` carrying a `block` field stand for here.
const BLOCK_KINDS: [&str; 2] = ["block", "do_block"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["lambda", "call"]) {
        let Some(block) = block_of(node) else {
            continue;
        };
        // `node.lambda_or_proc?`, and separately `BlockNode#lambda?`, which is only about the
        // selector: `->` and `lambda` are lambdas, `proc` and `Proc.new` are procs.
        let Some(is_lambda) = lambda_or_proc(node, context) else {
            continue;
        };
        let Some(body) = sole_statement(block) else {
            continue;
        };
        if !returns_nil(body, context) {
            continue;
        }
        // `return` leaves the enclosing method when it runs inside a proc, so only a lambda's is
        // the same thing as an empty body.
        if body.kind_str() == "return" && !is_lambda {
            continue;
        }
        let type_name = if is_lambda { "lambda" } else { "proc" };
        let text = context.source.text();
        // `node.single_line?` on a block is about its braces, not about the call in front of it.
        let removed = if context.source.line_column(block.start_byte()).0
            == context.source.line_column(block.end_byte()).0
        {
            // `range_with_surrounding_space(body.source_range)`.
            support::final_pos(text, body.start_byte(), false, true, false)
                ..support::final_pos(text, body.end_byte(), true, true, false)
        } else {
            // `range_by_whole_lines(body.source_range, include_final_newline: true)`.
            whole_lines(body.byte_range(), context)
        };
        offenses.push(
            context
                .offense(
                    format!("Use an empty {type_name} instead of always returning nil."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: removed.start,
                    end: removed.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// The block written on the node, when there is one.
fn block_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let block = match node.kind_str() {
        "lambda" => node.field("body")?,
        _ => node.field("block")?,
    };
    BLOCK_KINDS.contains(&block.kind_str()).then_some(block)
}

/// `lambda_or_proc?`, answering `BlockNode#lambda?` at the same time: `Some(true)` for `->` and
/// `lambda`, `Some(false)` for `proc` and `Proc.new`, `None` for anything else.
fn lambda_or_proc(node: Node<'_>, context: &RuleContext<'_>) -> Option<bool> {
    if node.kind_str() == "lambda" {
        return Some(true);
    }
    let selector = node.field("method")?;
    let name = context.source.node_text(selector);
    match (node.field("receiver"), name) {
        // `(send nil? :lambda)` / `(send nil? :proc)`.
        (None, "lambda") => Some(true),
        (None, "proc") => Some(false),
        // `(send #global_const?(:Proc) :new)`.
        (Some(receiver), "new") if send_node::top_level_constant(receiver, "Proc", context) => {
            Some(false)
        }
        _ => None,
    }
}

/// `node.body` when the block holds exactly one statement. Two or more make a `begin` upstream,
/// which is not one of the nodes the pattern accepts.
fn sole_statement<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let body = block.field("body")?;
    let statements: Vec<Node<'tree>> = super::nodes::children(body)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    match statements.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `{ ({return next break} nil) (nil) }`.
fn returns_nil(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "nil" => true,
        "return" | "next" | "break" => {
            let arguments = super::nodes::children(node);
            let [list] = arguments.as_slice() else {
                return false;
            };
            if list.kind_str() != "argument_list" {
                return false;
            }
            let _ = context;
            matches!(super::nodes::children(*list).as_slice(), [only] if only.kind_str() == "nil")
        }
        _ => false,
    }
}

/// `range_by_whole_lines(range, include_final_newline: true)`.
fn whole_lines(range: std::ops::Range<usize>, context: &RuleContext<'_>) -> std::ops::Range<usize> {
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |offset| range.end + offset + 1);
    start..end
}
