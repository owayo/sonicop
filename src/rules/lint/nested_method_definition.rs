use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::top_level_constant;

use super::blocks::BLOCK_KINDS;
use super::literals::is_constant;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Method definitions must not be nested. Use `lambda` instead.";

/// The blocks that open a scope of their own, so that a definition inside one is not a definition
/// inside the method the block was written in.
const SCOPING_METHODS: &[&str] = &[
    "instance_eval",
    "class_eval",
    "module_eval",
    "instance_exec",
    "class_exec",
    "module_exec",
];

/// `class_constructor?`: the calls that build a class or a module from a block.
const CONSTRUCTORS: &[(&str, &str)] = &[
    ("Class", "new"),
    ("Module", "new"),
    ("Struct", "new"),
    ("Data", "define"),
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // `def obj.name` defines a method on something else, so it is only nested when the thing
        // it defines on cannot be named from outside -- which is what `self` alone is.
        if node.kind_str() == "singleton_method"
            && node
                .field("object")
                .is_some_and(|subject| is_allowed_subject(subject, context))
        {
            continue;
        }
        if !has_enclosing_definition(node) {
            continue;
        }
        if opens_its_own_scope(node, context, &allowed) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// `allowed_subject_type?`: a variable, a constant or a call, all of which name something the
/// enclosing method does not own. `self` is none of the three, which is what makes
/// `def self.name` inside a method a nested definition.
fn is_allowed_subject(subject: Node<'_>, context: &RuleContext<'_>) -> bool {
    if subject.kind_str() == "identifier" {
        // The grammar spells the three keyword literals as identifiers here, and none of them is a
        // variable, a constant or a call; every other bare name is one of the three.
        return !matches!(context.source.node_text(subject), "nil" | "true" | "false");
    }
    matches!(
        subject.kind_str(),
        "instance_variable" | "class_variable" | "global_variable" | "call"
    ) || is_constant(subject, context)
}

fn has_enclosing_definition(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "method" | "singleton_method") {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

/// `each_ancestor(:any_block, :sclass).any? { |a| scoping_method_call?(a) }`.
fn opens_its_own_scope(node: Node<'_>, context: &RuleContext<'_>, allowed: &[String]) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind_str() == "singleton_class" {
            return true;
        }
        if BLOCK_KINDS.contains(&ancestor.kind_str()) && is_scoping_block(ancestor, context, allowed) {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

fn is_scoping_block(block: Node<'_>, context: &RuleContext<'_>, allowed: &[String]) -> bool {
    let Some(call) = block.parent().filter(|call| call.kind_str() == "call") else {
        return false;
    };
    let Some(selector) = call.field("method") else {
        return false;
    };
    let name = context.source.node_text(selector);
    if SCOPING_METHODS.contains(&name) || allowed.iter().any(|method| method == name) {
        return true;
    }
    call.field("receiver")
        .is_some_and(|receiver| {
            CONSTRUCTORS.iter().any(|(constant, method)| {
                name == *method && top_level_constant(receiver, constant, context)
            })
        })
}
