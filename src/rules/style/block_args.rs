//! Whether a block takes its parameters implicitly, which changes the node type upstream builds
//! for it.
//!
//! A block written without bars but reading `_1` is a `numblock`; one reading `it` is an
//! `itblock`. tree-sitter has one `block` node for all three and leaves `_1` and `it` as plain
//! identifiers in the body, so a cop whose handler upstream is `on_block` alone has to work out
//! which of the three it is looking at before it may report.

use tree_sitter::Node;

use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The version that made `_1` a block parameter rather than a receiverless call.
const NUMBERED_VERSION: RubyVersion = RubyVersion::new(2, 7);

/// The version that made `it` a block parameter rather than a receiverless call.
const IT_VERSION: RubyVersion = RubyVersion::new(3, 4);

/// Whether upstream reads this block as a `numblock` or an `itblock` rather than as a `block`.
pub(super) fn implicit(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    if block.field("parameters").is_some() {
        return false;
    }
    let Some(body) = block.field("body") else {
        return false;
    };
    let mut numbered = false;
    let mut it = false;
    scan(context, body, &mut numbered, &mut it);
    (numbered && context.target_ruby_version() >= NUMBERED_VERSION)
        || (it && context.target_ruby_version() >= IT_VERSION)
}

/// The part of the body that belongs to this block: a nested block's `_1` is that block's, not
/// this one's.
fn scan(context: &RuleContext<'_>, node: Node<'_>, numbered: &mut bool, it: &mut bool) {
    for child in super::nodes::children(node) {
        if matches!(child.kind_str(), "block" | "do_block" | "lambda") {
            continue;
        }
        if child.kind_str() == "identifier" {
            match context.source.node_text(child) {
                name if is_numbered_parameter(name) => *numbered = true,
                "it" => *it = true,
                _ => {}
            }
            continue;
        }
        scan(context, child, numbered, it);
    }
}

/// `_1` through `_9`, which is what the parser accepts as a numbered parameter.
fn is_numbered_parameter(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2 && bytes[0] == b'_' && bytes[1].is_ascii_digit() && bytes[1] != b'0'
}
