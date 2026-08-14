//! `Style/FileOpen`: a `File.open` nobody closes leaks the descriptor it opened.

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;
use crate::rules::support::value_used;

const MSG: &str = "`File.open` without a block may leak a file descriptor; use the block form.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(selector), Some(receiver)) = (node.field("method"), node.field("receiver"))
        else {
            continue;
        };
        if context.source.node_text(selector) != "open"
            || !super::nodes::is_top_level_constant(receiver, "File", context)
        {
            continue;
        }
        // `node.block_argument?`: `File.open(path, &:read)` hands the descriptor to something that
        // closes it.
        if arguments(node)
            .last()
            .is_some_and(|last| last.first().kind_str() == "block_argument")
        {
            continue;
        }
        // A block written after the call is a `block` node upstream, so `node.parent` is that
        // block: `value_used?` answers true through it and its receiver is the constant rather
        // than the call, which is how the block form escapes both tests. Here the block sits
        // inside the call, so the same answer comes from asking whether it has one.
        if node.field("block").is_some() {
            continue;
        }
        // `offensive_usage?`: a result nobody reads, one stored in a local variable, and one whose
        // only use is a further call are the three shapes with nothing left to close the file.
        let offensive = !value_used(context, node)
            || context.parent(node).is_some_and(|parent| {
                is_local_assignment(parent, node)
                    || parent
                        .field("receiver")
                        .is_some_and(|inner| inner.id() == node.id())
            });
        if offensive {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}

/// `node.parent.lvasgn_type?`: only a plain local variable, and only where the call is the value.
/// An `x ||= File.open(...)` is an `or_asgn` upstream, whose child is the `lvasgn`, so the call's
/// parent is not one.
fn is_local_assignment(parent: tree_sitter::Node<'_>, node: tree_sitter::Node<'_>) -> bool {
    parent.kind_str() == "assignment"
        && parent
            .field("left")
            .is_some_and(|left| left.kind_str() == "identifier")
        && parent
            .field("right")
            .is_some_and(|right| right.id() == node.id())
}
