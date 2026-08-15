use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `Dir.home` instead.";

/// `(send (const {cbase nil?} :ENV) {:[] :fetch} (str "HOME") ...)`.
///
/// Upstream catches both spellings with one pattern because `ENV['HOME']` and `ENV.fetch('HOME')`
/// are both `send` nodes to it. Here they are different nodes -- `element_reference` and `call` --
/// so the two are walked separately.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("element_reference") {
        // The receiver and the subscript are the two named children. A multi-index read
        // (`ENV['A', 'B']`) does not match upstream's pattern either, so only the single
        // subscript form is considered.
        let parts = super::nodes::children(node);
        let [object, index] = parts.as_slice() else {
            continue;
        };
        // Writing to the variable is an `indexasgn` upstream, which the pattern's `{:[] :fetch}`
        // never matches. The grammar writes the target with the same node a read gets, so the
        // assignment around it is what tells them apart.
        if is_assignment_target(node, context) {
            continue;
        }
        if is_env(*object, context) && is_home(*index, context) {
            offenses.push(offense(context, node));
        }
    }

    for node in context.nodes_of("call") {
        // `node.block_node`: a block changes what the second argument means, so `fetch` with one
        // is left alone.
        if node.field("block").is_some() {
            continue;
        }
        let (Some(receiver), Some(method)) = (node.field("receiver"), node.field("method")) else {
            continue;
        };
        if !is_env(receiver, context) || context.source.node_text(method) != "fetch" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        // Only `nil` is accepted as the default: any other one makes the call something
        // `Dir.home` does not stand for.
        let matched = match arguments.as_slice() {
            [first] => is_home(*first, context),
            [first, second] => is_home(*first, context) && second.kind_str() == "nil",
            _ => false,
        };
        if matched {
            offenses.push(offense(context, node));
        }
    }
}

/// Whether the read stands where an assignment puts what it writes to.
///
/// Only a plain assignment counts. `ENV['HOME'] += x` keeps the read as a read upstream -- the
/// operator assignment is written around it -- so that one still matches the pattern.
fn is_assignment_target(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        parent.kind_str() == "assignment"
            && parent
                .field("left")
                .is_some_and(|left| left.id() == node.id())
    })
}

/// `(const {cbase nil?} :ENV)`: the bare `ENV` and `::ENV`, but not `Foo::ENV`.
fn is_env(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    super::nodes::is_top_level_constant(node, "ENV", context)
}

/// `(str "HOME")`: a plain string holding exactly `HOME`, with no interpolation.
fn is_home(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "string" || node.named_child_count() != 1 {
        return false;
    }
    node.named_child(0).is_some_and(|content| {
        content.kind_str() == "string_content" && context.source.node_text(content) == "HOME"
    })
}

fn offense(context: &RuleContext<'_>, node: Node<'_>) -> Offense {
    context.offense(MSG, node.byte_range()).corrected_by(Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: "Dir.home".to_owned(),
        safe: true,
    })
}
