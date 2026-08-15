use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `rfind` instead.";

/// `minimum_target_ruby_version 4.0`: `Enumerable#rfind` arrives in Ruby 4.0.
const MINIMUM: RubyVersion = RubyVersion::new(4, 0);

/// `(call (call _ {:reverse :reverse_each}) {:find :detect} (block_pass sym)?)`.
///
/// The inner receiver is `_`, which also stands for no receiver at all, so a bare
/// `reverse.find { ... }` is a match.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if !matches!(context.source.node_text(selector), "find" | "detect") {
            continue;
        }
        // `(block_pass sym)?`: nothing, or exactly one `&:sym`. A block written with braces is a
        // separate node upstream and does not count as an argument.
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        match arguments.as_slice() {
            [] => {}
            [only] if is_symbol_block_pass(*only) => {}
            _ => continue,
        }
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let Some(inner) = reversing_selector(receiver, context, &locals) else {
            continue;
        };
        // `receiver.loc.selector.join(node.loc.selector)`: the two selectors and the dot between
        // them collapse into one.
        let range = inner.start_byte()..selector.end_byte();
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: "rfind".to_owned(),
            safe: true,
        }));
    }
}

/// `(block_pass sym)`: `&:name`, and not `&blk`.
fn is_symbol_block_pass(node: tree_sitter::Node<'_>) -> bool {
    node.kind_str() == "block_argument"
        && matches!(
            super::nodes::children(node).as_slice(),
            [only] if matches!(only.kind_str(), "simple_symbol" | "delimited_symbol")
        )
}

/// The `reverse` or `reverse_each` selector of the receiver, when the receiver is a call to one of
/// them.
///
/// A receiverless `reverse` is a bare `identifier` here but a `(send nil :reverse)` upstream, which
/// the pattern's `_` receiver matches. A local variable that happens to be named `reverse` is a
/// variable read and is not a call at all.
fn reversing_selector<'tree>(
    receiver: Node<'tree>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Node<'tree>> {
    let selector = match receiver.kind_str() {
        "call" => {
            if receiver.field("block").is_some() {
                return None;
            }
            receiver.field("method")?
        }
        "identifier" if !locals.is_lvar(receiver) => receiver,
        _ => return None,
    };
    matches!(context.source.node_text(selector), "reverse" | "reverse_each").then_some(selector)
}
