//! What a block declares between its bars, in the three node types upstream's parser builds.
//!
//! A block written with `|x|` is a `block` holding an `args` list; one written without bars but
//! reading `_1` is a `numblock` whose arity is the highest number it reads; one reading `it` is an
//! `itblock`. tree-sitter has a single `block` node for all three and leaves `_1` and `it` as plain
//! identifiers in the body, so the type a node pattern names has to be worked out from what the
//! body holds -- and from the target version, since `it` only became a parameter in Ruby 3.4.

use tree_sitter::Node;

use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::named_children;

use super::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// The kinds tree-sitter writes a block as. A `lambda` is `-> { }`, which upstream also reaches
/// through `on_block`.
pub(crate) const BLOCK_KINDS: &[&str] = &["block", "do_block"];

/// The version that made `_1` a block parameter rather than a receiverless call.
const NUMBERED_VERSION: RubyVersion = RubyVersion::new(2, 7);

/// The version that made `it` a block parameter rather than a receiverless call.
const IT_VERSION: RubyVersion = RubyVersion::new(3, 4);

/// The parameters of a block, as the node type upstream builds for it.
pub(crate) enum BlockArgs<'tree> {
    /// `(block _ (args ...) _)`: what was written between the bars, which may be nothing.
    Written(Vec<Node<'tree>>),
    /// `(numblock _ n _)`: the highest `_n` the body reads.
    Numbered(usize),
    /// `(itblock _ :it _)`.
    It,
}

impl<'tree> BlockArgs<'tree> {
    pub(crate) fn of(
        block: Node<'tree>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) -> Self {
        if let Some(parameters) = block.field("parameters") {
            return Self::Written(
                named_children(parameters)
                    .into_iter()
                    .filter(|child| child.kind_str() != "comment")
                    .collect(),
            );
        }
        let Some(body) = block.field("body") else {
            return Self::Written(Vec::new());
        };
        // A numbered parameter and an `it` belong to the innermost block around them, so a nested
        // block's are not this one's. Both are ordinary receiverless calls in the versions before
        // the one that gave them a meaning, which leaves the block a `block` taking no arguments.
        let mut highest = 0;
        let mut it = false;
        scan(body, context, locals, &mut highest, &mut it);
        if highest > 0 && context.target_ruby_version() >= NUMBERED_VERSION {
            return Self::Numbered(highest);
        }
        if it && context.target_ruby_version() >= IT_VERSION {
            return Self::It;
        }
        Self::Written(Vec::new())
    }

    /// `(args (arg _))`: exactly one plain required parameter.
    pub(crate) fn single_plain_arg(&self) -> bool {
        matches!(self, Self::Written(params) if params.len() == 1 && params[0].kind_str() == "identifier")
    }

    /// `(args)`: no parameters written at all.
    pub(crate) fn none(&self) -> bool {
        matches!(self, Self::Written(params) if params.is_empty())
    }
}

/// The highest numbered parameter and whether `it` is read, over the part of the body that belongs
/// to this block.
fn scan(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    highest: &mut usize,
    it: &mut bool,
) {
    for child in named_children(node) {
        if BLOCK_KINDS.contains(&child.kind_str()) || child.kind_str() == "lambda" {
            continue;
        }
        if child.kind_str() == "identifier" {
            let text = context.source.node_text(child);
            if let Some(number) = numbered_parameter(text) {
                *highest = (*highest).max(number);
            } else if text == "it" && !locals.is_lvar(child) {
                *it = true;
            }
            continue;
        }
        scan(child, context, locals, highest, it);
    }
}

/// `_1` through `_9`, which is what the parser accepts as a numbered parameter.
fn numbered_parameter(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() != 2 || bytes[0] != b'_' || !bytes[1].is_ascii_digit() || bytes[1] == b'0' {
        return None;
    }
    Some((bytes[1] - b'0') as usize)
}
