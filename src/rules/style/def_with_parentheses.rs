use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Omit the parentheses in defs when the method doesn't accept any arguments.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        // `!node.arguments?`: the list has to be empty, and `node.arguments.source_range` has to
        // exist, which for an empty list means the parentheses were written.
        if !super::nodes::children(parameters).is_empty()
            || !context.source.node_text(parameters).starts_with('(')
        {
            continue;
        }
        if parentheses_required(context, node, parameters) {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, parameters.byte_range())
                .corrected_by(Edit {
                    start: parameters.start_byte(),
                    end: parameters.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `parentheses_required?`: without them `def foo do_something end` would not parse, and neither
/// would `def foo()=x`.
fn parentheses_required(context: &RuleContext<'_>, node: Node<'_>, parameters: Node<'_>) -> bool {
    let after = context
        .source
        .text()
        .as_bytes()
        .get(parameters.end_byte())
        .copied();
    // A `;` already separates the signature from the body.
    if after == Some(b';') {
        return false;
    }
    let single_line = node.start_position().row == node.end_position().row;
    let endless = !node
        .child(node.child_count().saturating_sub(1) as u32)
        .is_some_and(|last| last.kind_str() == "end");
    if single_line && !endless {
        return true;
    }
    after == Some(b'=')
}
