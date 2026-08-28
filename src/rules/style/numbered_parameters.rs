use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::blocks::{BLOCK_KINDS, BlockArgs};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const MSG_DISALLOW: &str = "Avoid using numbered parameters.";
const MSG_MULTI_LINE: &str = "Avoid using numbered parameters for multi-line blocks.";

/// `minimum_target_ruby_version 2.7`: before that `_1` is an ordinary method call.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);

/// `on_numblock`: a block that reads `_1` instead of naming its parameters.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let disallow = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "disallow");
    let locals = LocalVariables::new(context);
    // **A lambda literal is a `block` upstream too.** `-> { _1 }` is a `numblock` whose send is
    // the `lambda` call, while the grammar gives the arrow a node of its own that holds the block.
    for node in context.nodes_of_any(&["call", "lambda"]) {
        let Some(block) = node
            .field("block")
            .or_else(|| (node.kind_str() == "lambda").then(|| node.field("body")).flatten())
            .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        else {
            continue;
        };
        if !matches!(BlockArgs::of(block, context, &locals), BlockArgs::Numbered(_)) {
            continue;
        }
        if disallow {
            offenses.push(context.offense(MSG_DISALLOW, node.byte_range()));
        } else if context.source.line_column(block.start_byte()).0
            != context.source.line_column(block.end_byte()).0
        {
            // `node.multiline?` on a block is about its braces, not about the call in front of it.
            offenses.push(context.offense(MSG_MULTI_LINE, node.byte_range()));
        }
    }
}
