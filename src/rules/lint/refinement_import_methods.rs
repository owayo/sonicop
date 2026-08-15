use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::ruby_version::RubyVersion;

use super::statements::statements;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < RubyVersion::new(3, 1) {
        return;
    }
    // `import_methods` was only deprecated in 3.1; from 3.2 the two are gone outright.
    let template = if context.target_ruby_version() >= RubyVersion::new(3, 2) {
        "it was removed in Ruby 3.2"
    } else {
        "it is deprecated in Ruby 3.1"
    };
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !matches!(name, "include" | "prepend") || node.field("receiver").is_some() {
            continue;
        }
        if !is_sole_statement_of_refine_block(node, context) {
            continue;
        }
        let message =
            format!("Use `import_methods` instead of `{name}` because {template}.");
        let range = selector.byte_range();
        offenses.push(
            context
                .offense(message, range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: "import_methods".to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `parent.block_type? && parent.method?(:refine)`.
///
/// Upstream a block holding one statement *is* that statement, so the call reaches the `block`
/// through `parent` only while it stands alone. The grammar always puts a body node in between,
/// which is why the count has to be checked here.
fn is_sole_statement_of_refine_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(body) = node.parent_of(context) else {
        return false;
    };
    if !matches!(body.kind_str(), "body_statement" | "block_body") {
        return false;
    }
    if statements(body).len() != 1 {
        return false;
    }
    let Some(block) = body.parent_of(context) else {
        return false;
    };
    if !matches!(block.kind_str(), "block" | "do_block") {
        return false;
    }
    block
        .parent_of(context)
        .and_then(|call| call.field("method"))
        .is_some_and(|method| context.source.node_text(method) == "refine")
}
