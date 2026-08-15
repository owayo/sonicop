use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::top_level_constant;

use super::statements::statements;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let only: Vec<String> = context.setting("Only").unwrap_or_default();
    let ignore: Vec<String> = context.setting("Ignore").unwrap_or_default();
    for node in context.nodes_of("constant") {
        // `(const nil? _)`: a name written with nothing in front of it. Anything reached through a
        // scope is already qualified, and the target of a constant assignment is a `casgn`.
        if !is_unqualified_read(node, context) {
            continue;
        }
        let name = context.source.node_text(node);
        if (!only.is_empty() && !only.iter().any(|allowed| allowed == name))
            || ignore.iter().any(|ignored| ignored == name)
        {
            continue;
        }
        // `node.parent&.defined_module`: the name a `class`, `module` or `X = Class.new` gives to
        // what it defines is not a lookup.
        if defining_parent(node, context).is_some_and(|parent| defines_module(parent, context)) {
            continue;
        }
        offenses.push(context.offense(
            "Fully qualify this constant to avoid possibly ambiguous resolution.",
            node.byte_range(),
        ));
    }
}

fn is_unqualified_read(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return true;
    };
    match parent.kind_str() {
        // The name after `::` is no node of its own, and the scope in front of it *is* the
        // unqualified constant.
        "scope_resolution" => parent
            .field("name")
            .is_none_or(|name| name.id() != node.id()),
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_none_or(|left| left.id() != node.id()),
        // A method whose name starts with a capital, as `Rainbow('...')` or `Integer(x)` do, is a
        // `send` upstream and no constant at all. The grammar writes its name with the same node it
        // writes a constant with, so only the name is not a lookup -- a constant standing there as
        // the receiver still is one.
        "call" => parent
            .field("method")
            .is_none_or(|method| method.id() != node.id()),
        _ => true,
    }
}

/// `node.parent`, as upstream's tree has it.
///
/// A superclass hangs off a node of its own here, and a body holding one statement *is* that
/// statement upstream -- both of which change which node a constant's parent is.
fn defining_parent<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context)?;
    loop {
        current = match current.kind_str() {
            "superclass" => current.parent_of(context)?,
            "body_statement" | "block_body" if statements(current).len() == 1 => {
                current.parent_of(context)?
            }
            // The scope of a constant assignment target belongs to the `casgn` upstream.
            "scope_resolution" => {
                let parent = current.parent_of(context)?;
                let is_target = matches!(parent.kind_str(), "assignment" | "operator_assignment")
                    && parent
                        .field("left")
                        .is_some_and(|left| left.id() == current.id());
                if !is_target {
                    return Some(current);
                }
                parent
            }
            _ => return Some(current),
        };
    }
}

/// `defined_module0`: a `class`, a `module`, or a constant assigned `Class.new` / `Module.new`.
fn defines_module(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "class" | "module" => true,
        "assignment" => node
            .field("right")
            .is_some_and(|right| is_class_or_module_new(right, context)),
        _ => false,
    }
}

fn is_class_or_module_new(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
        return false;
    };
    context.source.node_text(method) == "new"
        && (top_level_constant(receiver, "Class", context)
            || top_level_constant(receiver, "Module", context))
}
