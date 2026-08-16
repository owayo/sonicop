use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, top_level_constant};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        // `on_send` は `csend` に呼ばれない。`alias on_csend on_send` を書いていない cop は
        // `x&.foo` を構造的に一切見ないので、ここで落とさないと過剰検出になる。
        if !crate::rules::send_node::is_plain_send(node, context) {
            continue;
        }
        if context.source.node_text(method) != "select"
            || !top_level_constant(receiver, "IO", context)
        {
            continue;
        }
        let call_arguments = arguments(node);
        let argument = |index: usize| call_arguments.get(index).map(|argument| argument.first());
        let (read, write, excepts, timeout) = (
            argument(0),
            argument(1),
            argument(2),
            argument(3),
        );
        // `excepts && !excepts.children.empty?`: a list of exceptional streams has no equivalent.
        if excepts.is_some_and(|excepts| !values(excepts).is_empty()) {
            continue;
        }
        if !(scheduler_compatible(read, write) || scheduler_compatible(write, read)) {
            continue;
        }
        let Some(preferred) = preferred_method(read, write, timeout, context) else {
            continue;
        };
        let range = node.byte_range();
        let message = format!(
            "Use `{preferred}` instead of `{}`.",
            context.source.slice(range.clone())
        );
        let offense = context.offense(message, range.clone());
        // `node.parent&.assignment?`: the call's value is being kept, and the replacement has a
        // different one.
        offenses.push(if is_assigned(node, context) {
            offense
        } else {
            offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: preferred,
                safe: true,
            })
        });
    }
}

/// `scheduler_compatible?`: one stream to wait on, and nothing to wait on in the other direction.
fn scheduler_compatible(io1: Option<Node<'_>>, io2: Option<Node<'_>>) -> bool {
    if !io1.is_some_and(single_io_array) {
        return false;
    }
    match io2 {
        Some(node) if node.kind_str() == "array" => values(node).is_empty(),
        Some(node) => node.kind_str() == "nil",
        None => true,
    }
}

fn single_io_array(node: Node<'_>) -> bool {
    node.kind_str() == "array"
        && match values(node).as_slice() {
            [only] => only.kind_str() != "splat_argument",
            _ => false,
        }
}

fn values<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

/// `preferred_method`: the wait the single stream stands for.
fn preferred_method(
    read: Option<Node<'_>>,
    write: Option<Node<'_>>,
    timeout: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<String> {
    let argument = timeout.map_or_else(String::new, |timeout| {
        format!("({})", context.source.node_text(timeout))
    });
    let readable = read.filter(|read| read.kind_str() == "array");
    if let Some(first) = readable.and_then(|read| values(read).first().copied()) {
        return Some(format!(
            "{}.wait_readable{argument}",
            context.source.node_text(first)
        ));
    }
    let first = values(write?).first().copied()?;
    Some(format!(
        "{}.wait_writable{argument}",
        context.source.node_text(first)
    ))
}

/// `Node#assignment?`: the eight assignment node types, which is not the same question
/// `SendNode#assignment?` answers.
fn is_assigned(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        matches!(parent.kind_str(), "assignment" | "operator_assignment")
            && parent
                .field("right")
                .is_some_and(|right| right.id() == node.id())
    })
}
