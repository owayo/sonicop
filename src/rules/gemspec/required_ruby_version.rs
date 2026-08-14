use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{
    FILE_KEYWORD, arguments, is_plain_send, is_string, named_children, string_text,
};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let assignments: Vec<Node<'_>> = context
        .nodes_of("assignment")
        .filter(|node| assigns_required_ruby_version(*node, context))
        .collect();

    if assignments.is_empty() {
        // `add_global_offense`, which upstream anchors at the head of the file because there is no
        // syntax to point at: the offense is that nothing was written.
        offenses.push(context.offense("`required_ruby_version` should be specified.", 0..0));
        return;
    }

    let target = context.target_ruby_version().to_string();
    for node in assignments {
        let Some(version) = node.field("right") else {
            continue;
        };
        if dynamic_version(version, context) {
            continue;
        }
        if declared_version(version, context).as_deref() == Some(target.as_str()) {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "`required_ruby_version` and `TargetRubyVersion` ({target}, which may be \
                 specified in .rubocop.yml) should be equal."
            ),
            version.byte_range(),
        ));
    }
}

/// `(send _ :required_ruby_version= _)`. A bare `required_ruby_version = ...` assigns a local
/// variable rather than the specification, so the assignment has to go through a receiver.
fn assigns_required_ruby_version(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(left) = node.field("left") else {
        return false;
    };
    left.kind_str() == "call"
        && is_plain_send(left, context)
        && left
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "required_ruby_version")
}

/// `dynamic_version?`: a version the gemspec works out as it loads cannot be compared with a
/// version written in a configuration file, so upstream leaves it alone. Anything holding a method
/// call or a variable, at any depth, counts.
fn dynamic_version(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    // The version itself only counts when it is a variable or a call made without a receiver --
    // `Gem::Requirement.new(...)` is a call and still readable -- but *within* it, any call or
    // variable at any depth is enough.
    if is_variable(node, context)
        || (is_send(node, context) && node.field("receiver").is_none())
    {
        return true;
    }
    named_children(node)
        .into_iter()
        .any(|child| dynamic_descendant(child, context))
}

fn dynamic_descendant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    is_variable(node, context)
        || is_send(node, context)
        || named_children(node)
            .into_iter()
            .any(|child| dynamic_descendant(child, context))
}

fn is_send(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call" && is_plain_send(node, context)
}

/// `RuboCop::AST::Node::VARIABLES`, plus the bare name upstream's parser has already resolved into
/// either a local variable or a receiverless call. Which of the two it is does not matter -- both
/// are dynamic -- but the name a call is made *through* is no node of its own there.
fn is_variable(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "instance_variable" | "class_variable" | "global_variable" => true,
        // `__FILE__` is not a name at all by the time a cop sees it: the parser has already put the
        // path it stood for in its place.
        "identifier" => {
            context.source.node_text(node) != FILE_KEYWORD
                && node.parent_of(context).is_none_or(|parent| {
                    parent.field("method") != Some(node)
                        && parent.field("name") != Some(node)
                })
        }
        _ => false,
    }
}

/// The `major.minor` version the gemspec asks for, taken the way `extract_ruby_version` does: the
/// first requirement that carries a `>` or an `=`, reduced to its first two digits.
fn declared_version(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let requirement = match requirements(node, context)? {
        Requirements::One(node) => node,
        Requirements::Many(nodes) => nodes
            .into_iter()
            .find(|node| string_text(*node, context).contains(['>', '=']))?,
    };
    let digits: Vec<String> = string_text(requirement, context)
        .chars()
        .filter(char::is_ascii_digit)
        .take(2)
        .map(String::from)
        .collect();
    Some(digits.join("."))
}

enum Requirements<'tree> {
    One(Node<'tree>),
    Many(Vec<Node<'tree>>),
}

/// `defined_ruby_version`: a string, a two-element array of strings, or `Gem::Requirement.new` over
/// strings. Anything else -- including an array of any other length -- names no version at all.
fn requirements<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Requirements<'tree>> {
    if is_string(node, context) {
        return Some(Requirements::One(node));
    }
    if node.kind_str() == "array" {
        let elements = named_children(node);
        return match elements.len() == 2 && elements.iter().all(|node| is_string(*node, context)) {
            true => Some(Requirements::Many(elements)),
            false => None,
        };
    }
    if node.kind_str() == "call" && gem_requirement_new(node, context) {
        let elements: Vec<Node<'tree>> = arguments(node)
            .iter()
            .map(|argument| argument.first())
            .collect();
        return match !elements.is_empty() && elements.iter().all(|node| is_string(*node, context)) {
            true => Some(Requirements::Many(elements)),
            false => None,
        };
    }
    None
}

/// `(send (const (const nil? :Gem) :Requirement) :new ...)`. Note the missing `cbase`: upstream
/// spells the outer scope `nil?` here, so `::Gem::Requirement` is not this constant.
fn gem_requirement_new(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "new")
    {
        return false;
    }
    let Some(receiver) = node.field("receiver") else {
        return false;
    };
    receiver.kind_str() == "scope_resolution"
        && receiver
            .field("name")
            .is_some_and(|name| context.source.node_text(name) == "Requirement")
        && receiver.field("scope").is_some_and(|scope| {
            scope.kind_str() == "constant" && context.source.node_text(scope) == "Gem"
        })
}
