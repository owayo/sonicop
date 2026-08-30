//! `Style/EmptyClassDefinition`: one way of writing a class that adds nothing of its own.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

const MSG_CLASS_KEYWORD: &str =
    "Use the `class` keyword instead of `Class.new` to define an empty class.";
const MSG_CLASS_NEW: &str =
    "Use `Class.new` instead of the `class` keyword to define an empty class.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "class_keyword".to_owned());
    let allowed: Vec<String> = context.setting("AllowedParentClasses").unwrap_or_default();
    if matches!(style.as_str(), "class_keyword" | "class_definition") {
        for node in context.nodes_of("assignment") {
            check_assignment(context, node, &allowed, offenses);
        }
    }
    if style == "class_new" {
        for node in context.nodes_of("class") {
            check_class(context, node, &allowed, offenses);
        }
    }
}

/// `(casgn _ _ $(send (const _ :Class) :new _))`: a constant taking a class with one parent.
fn check_assignment(
    context: &RuleContext<'_>,
    node: Node<'_>,
    allowed: &[String],
    offenses: &mut Vec<Offense>,
) {
    let (Some(name), Some(value)) = (node.field("left"), node.field("right")) else {
        return;
    };
    if !matches!(name.kind_str(), "constant" | "scope_resolution") {
        return;
    }
    if value.kind_str() != "call" || value.field("block").is_some() {
        return;
    }
    // `(const _ :Class)` names any `Class`, not only the top-level one.
    let named = |node: Node<'_>, wanted: &str| match node.kind_str() {
        "constant" => context.source.node_text(node) == wanted,
        "scope_resolution" => node
            .field("name")
            .is_some_and(|inner| context.source.node_text(inner) == wanted),
        _ => false,
    };
    if value
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "new")
        || value.field("receiver").is_none_or(|r| !named(r, "Class"))
    {
        return;
    }
    let list = arguments(value);
    let [parent] = list.as_slice() else {
        return;
    };
    let parent = parent.first();
    if !matches!(parent.kind_str(), "constant" | "scope_resolution") {
        return;
    }
    let parent_name = context.source.node_text(parent);
    if allowed.iter().any(|entry| entry == parent_name) {
        return;
    }
    offenses.push(
        context
            .offense(MSG_CLASS_KEYWORD, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!(
                    "class {} < {parent_name}\n{}end",
                    context.source.node_text(name),
                    " ".repeat(node.start_position().column)
                ),
                safe: true,
            }),
    );
}

/// A `class` with a parent and no body, which the `class_new` style writes as an assignment.
fn check_class(
    context: &RuleContext<'_>,
    node: Node<'_>,
    allowed: &[String],
    offenses: &mut Vec<Offense>,
) {
    let (Some(name), Some(superclass)) = (node.field("name"), node.field("superclass")) else {
        return;
    };
    if node.field("body").is_some() {
        return;
    }
    let parts = super::nodes::children_in(superclass, context);
    let [parent] = parts.as_slice() else {
        return;
    };
    let parent_name = context.source.node_text(*parent);
    if allowed.iter().any(|entry| entry == parent_name) {
        return;
    }
    offenses.push(
        context
            .offense(MSG_CLASS_NEW, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!(
                    "{} = Class.new({parent_name})",
                    context.source.node_text(name)
                ),
                safe: true,
            }),
    );
}
