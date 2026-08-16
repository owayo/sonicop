use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::const_name;

const CONSTRUCTOR_MSG: &str = "Call `super` to initialize state of the parent class.";
const CALLBACK_MSG: &str = "Call `super` to invoke callback defined in the parent class.";

/// Classes that hold no state of their own, so a constructor has nothing to pass up to.
const STATELESS_CLASSES: &[&str] = &["BasicObject", "Object"];

/// The lifecycle hooks Ruby calls on the class itself, which shadow the parent's unless `super`
/// passes the call on.
const CALLBACKS: &[&str] = &[
    "inherited",
    "method_added",
    "method_removed",
    "method_undefined",
    "singleton_method_added",
    "singleton_method_removed",
    "singleton_method_undefined",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut allowed: Vec<String> = STATELESS_CLASSES.iter().map(|&name| name.into()).collect();
    allowed.extend(
        context
            .setting::<Vec<String>>("AllowedParentClasses")
            .unwrap_or_default(),
    );
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let name = node
            .field("name")
            .map_or("", |name| context.source.node_text(name));
        let callback = CALLBACKS.contains(&name) && inside_a_class(node);
        let constructor = node.kind_str() == "method" && name == "initialize";
        if !(callback || constructor) || contains_super(node) {
            continue;
        }
        let message = if constructor && stateful_parent(node, context, &allowed) {
            CONSTRUCTOR_MSG
        } else if callback {
            CALLBACK_MSG
        } else {
            continue;
        };
        offenses.push(context.offense(message, node.byte_range()));
    }
}

/// `callback_method_def?` also asks that the definition stand in a class, module or singleton
/// class: outside one there is no parent to have defined the callback.
fn inside_a_class(node: Node<'_>) -> bool {
    ancestor(node, &["class", "singleton_class", "module"]).is_some()
}

fn contains_super(node: Node<'_>) -> bool {
    let mut stack: Vec<Node<'_>> = Vec::new();
    push_children(node, &mut stack);
    while let Some(current) = stack.pop() {
        if current.kind_str() == "super" {
            return true;
        }
        push_children(current, &mut stack);
    }
    false
}

fn push_children<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    stack.extend(node.named_children(&mut cursor));
}

/// `inside_class_with_stateful_parent?`. A block ancestor is consulted before any class one, and
/// only `Class.new(Parent) do ... end` counts as naming a parent -- a definition written inside any
/// other block is left alone, because the class it ends up on is not visible here.
fn stateful_parent(node: Node<'_>, context: &RuleContext<'_>, allowed: &[String]) -> bool {
    if let Some(block) = ancestor(node, &["block", "do_block"]) {
        return class_new_superclass(block, context)
            .is_some_and(|superclass| !allowed_class(superclass, context, allowed));
    }
    let Some(class) = ancestor(node, &["class"]) else {
        return false;
    };
    class
        .field("superclass")
        .and_then(|superclass| superclass.named_child(0))
        .is_some_and(|superclass| !allowed_class(superclass, context, allowed))
}

/// The one argument of a `Class.new(Parent)` the block was written on.
fn class_new_superclass<'tree>(
    block: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let call = block.parent().filter(|parent| parent.kind_str() == "call")?;
    if context.source.node_text(call.field("method")?) != "new"
        || !top_level_class(call.field("receiver")?, context)
    {
        return None;
    }
    let arguments = call.field("arguments")?;
    (arguments.named_child_count() == 1).then(|| arguments.named_child(0))?
}

fn top_level_class(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == "Class",
        "scope_resolution" => {
            node.field("scope").is_none()
                && node
                    .field("name")
                    .is_some_and(|name| context.source.node_text(name) == "Class")
        }
        _ => false,
    }
}

/// `allowed_class?`, which reads the parent's name the way `Node#const_name` does. A parent that is
/// not a constant at all has no name to match, so it counts as stateful.
fn allowed_class(node: Node<'_>, context: &RuleContext<'_>, allowed: &[String]) -> bool {
    const_name(node, context).is_some_and(|name| allowed.contains(&name))
}

fn ancestor<'tree>(node: Node<'tree>, kinds: &[&str]) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(found) = current {
        if kinds.contains(&found.kind_str()) {
            return Some(found);
        }
        current = found.parent();
    }
    None
}
