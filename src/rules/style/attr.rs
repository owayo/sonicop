use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::is_plain_send;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        // `node.command?(:attr)`: no receiver, and the name spelled out.
        if node.field("receiver").is_some() {
            continue;
        }
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "attr" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        if arguments.is_empty() || allowed_context(context, node) {
            continue;
        }
        // `replacement_method`: only a trailing boolean says which of the two was meant.
        let replacement = match arguments.last().map(|last| context.source.node_text(*last)) {
            Some("true") => "attr_accessor",
            _ => "attr_reader",
        };
        let mut edits = vec![Edit {
            start: selector.start_byte(),
            end: selector.end_byte(),
            replacement: replacement.to_owned(),
            safe: true,
        }];
        // The boolean is dropped only when it is the second argument, whatever the last one is.
        if arguments
            .get(1)
            .is_some_and(|second| matches!(context.source.node_text(*second), "true" | "false"))
        {
            edits.push(Edit {
                start: arguments[0].end_byte(),
                end: node.end_byte(),
                replacement: String::new(),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(
                    format!("Do not use `attr`. Use `{replacement}` instead."),
                    selector.byte_range(),
                )
                .corrected_by_all(edits),
        );
    }
}

/// `allowed_context?`: only a class body, or a `class_eval` block, is where `attr` means what this
/// cop thinks it means -- and even there a locally defined `attr` excuses it.
fn allowed_context(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(ancestor) = std::iter::successors(node.parent_of(context), |current| {
        current.parent_of(context)
    })
    .find(|current| matches!(current.kind_str(), "class" | "block" | "do_block")) else {
        return false;
    };
    if ancestor.kind_str() != "class" && !is_class_eval(context, ancestor) {
        return true;
    }
    defines_attr(context, ancestor)
}

/// `(block (send _ {:class_eval :module_eval}) ...)`: the grammar hangs the block off the call, so
/// the call is the block's parent rather than its first child.
///
/// The pattern is written for `send`, so `x&.class_eval { ... }` is a `csend` and does not match
/// it. That makes the block *not* a `class_eval` block, which is what keeps the cop quiet there.
fn is_class_eval(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    block.parent_of(context).is_some_and(|call| {
        is_plain_send(call, context)
            && call.field("method").is_some_and(|method| {
                matches!(
                    context.source.node_text(method),
                    "class_eval" | "module_eval"
                )
            })
    })
}

/// `each_descendant(:def).any? { |def_node| def_node.method?(:attr) }`.
fn defines_attr(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    super::conditional::descendants(node)
        .into_iter()
        .filter(|descendant| descendant.kind_str() == "method")
        .any(|definition| {
            definition
                .field("name")
                .is_some_and(|name| context.source.node_text(name) == "attr")
        })
}
