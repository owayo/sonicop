use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::blocks::{BLOCK_KINDS, BlockArgs};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 2.7`.
const MINIMUM: RubyVersion = RubyVersion::new(2, 7);

/// `DEFAULT_MAX_VALUE`, and the ceiling `max_count` clamps to.
const DEFAULT_MAX: usize = 1;
const CEILING: usize = 9;

/// `on_numblock`: how many distinct numbered parameters the block reads.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let max = context
        .setting::<usize>("Max")
        .unwrap_or(DEFAULT_MAX)
        .min(CEILING);
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(block) = node
            .field("block")
            .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        else {
            continue;
        };
        if !matches!(BlockArgs::of(block, context, &locals), BlockArgs::Numbered(_)) {
            continue;
        }
        // `numbered_parameter_nodes(node).uniq.count`: two reads of `_1` are structurally the same
        // node upstream, so `uniq` leaves the distinct names.
        let count = numbered_parameters(node, context).len();
        if count <= max {
            continue;
        }
        let parameter = if max > 1 { "parameters" } else { "parameter" };
        offenses.push(context.offense(
            format!("Avoid using more than {max} numbered {parameter}; {count} detected."),
            node.byte_range(),
        ));
    }
}

/// `each_descendant(:lvar)` filtered by `/\A_[1-9]\z/`.
fn numbered_parameters(node: Node<'_>, context: &RuleContext<'_>) -> HashSet<String> {
    let mut found = HashSet::new();
    let mut stack: Vec<Node<'_>> = super::nodes::children(node);
    while let Some(current) = stack.pop() {
        stack.extend(super::nodes::children(current));
        if current.kind_str() != "identifier" {
            continue;
        }
        let text = context.source.node_text(current);
        if !is_numbered(text) {
            continue;
        }
        // A name used as a selector or an assignment target is not a variable read.
        if is_field_of(current, "method", &["call"])
            || is_field_of(current, "left", &["assignment", "operator_assignment"])
        {
            continue;
        }
        found.insert(text.to_owned());
    }
    found
}

fn is_numbered(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2 && bytes[0] == b'_' && (b'1'..=b'9').contains(&bytes[1])
}

fn is_field_of(node: Node<'_>, field: &str, parents: &[&str]) -> bool {
    node.parent().is_some_and(|parent| {
        parents.contains(&parent.kind_str())
            && parent
                .field(field)
                .is_some_and(|value| value.id() == node.id())
    })
}
