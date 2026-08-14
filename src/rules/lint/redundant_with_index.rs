use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{is_plain_send, send_range};

use super::blocks::{BLOCK_KINDS, BlockArgs};
use super::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const MSG_EACH_WITH_INDEX: &str = "Use `each` instead of `each_with_index`.";
const MSG_WITH_INDEX: &str = "Remove redundant `with_index`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for call in context.nodes_of("call") {
        let Some(block) = call.field("block") else {
            continue;
        };
        if !BLOCK_KINDS.contains(&block.kind_str()) {
            continue;
        }
        let Some(selector) = call.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !matches!(name, "each_with_index" | "with_index") {
            continue;
        }
        // `node.receiver` is `{(send $_ ...) (any_block (call $_ ...) ...)}`, so the block's is the
        // receiver of the call it wraps -- and `with_index` is only redundant when the call it is
        // chained onto has a receiver of its own, since `each.with_index` may enumerate anything.
        let Some(receiver) = call.field("receiver") else {
            continue;
        };
        if name == "with_index" && !is_send_with_receiver(receiver, context) {
            continue;
        }
        // `{(args (arg _)) (args)}`, `numblock` of arity 1, or an `itblock`.
        let args = BlockArgs::of(block, context, &locals);
        let matched = match &args {
            BlockArgs::Numbered(arity) => *arity == 1,
            BlockArgs::It => true,
            BlockArgs::Written(_) => args.single_plain_arg() || args.none(),
        };
        if !matched {
            continue;
        }
        let range = selector.start_byte()..send_range(call, context).end;
        let offense = if name == "each_with_index" {
            context
                .offense(MSG_EACH_WITH_INDEX, range)
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: "each".to_owned(),
                    safe: true,
                })
        } else {
            let dot = call
                .field("operator")
                .map(|dot| Edit {
                    start: dot.start_byte(),
                    end: dot.end_byte(),
                    replacement: String::new(),
                    safe: true,
                })
                .into_iter();
            context
                .offense(MSG_WITH_INDEX, range.clone())
                .corrected_by_all(
                    [Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    }]
                    .into_iter()
                    .chain(dot),
                )
        };
        offenses.push(offense);
    }
}

/// Whether `node.receiver` answers anything, which the matcher
/// `{(send $_ ...) (any_block (call $_ ...) ...)}` only does for a call: a literal, a constant and
/// a variable all answer nothing. A `csend` answers nothing either unless a block was written
/// after it, since `any_block` wraps a `call` while the first alternative names `send` alone.
fn is_send_with_receiver(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && node.field("receiver").is_some()
        && (is_plain_send(node, context) || node.field("block").is_some())
}
