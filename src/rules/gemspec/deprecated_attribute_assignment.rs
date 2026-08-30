use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::{push_named_children_in};
use crate::rules::send_node::{arguments, is_plain_send, top_level_constant};
use crate::rules::support::whole_lines;
use crate::rules::send_node::named_children_of;

/// The attributes RubyGems deprecated, in the order upstream tries them. The order is what the cop
/// reports, and it is also load-bearing for `+=`: see [`deprecated_attribute`].
const DEPRECATED: &[&str] = &[
    "test_files",
    "date",
    "specification_version",
    "rubygems_version",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if !is_specification(call, context) {
            continue;
        }
        // `block_node.first_argument.source`. Upstream reads the parameter before it looks for an
        // assignment, so a specification opened without one takes the cop down with a `NoMethodError`
        // on `nil` and the file is reported with no offense at all.
        let Some(parameter) = block_parameter(call, context) else {
            continue;
        };
        // `block_node.descendants.detect`: the first assignment in the block, and only that one.
        let Some((assignment, attribute)) = first_deprecated(call, parameter, context) else {
            continue;
        };
        let removed = whole_lines(assignment.byte_range(), context);
        offenses.push(
            context
                .offense(
                    format!("Do not set `{attribute}` in gemspec."),
                    assignment.byte_range(),
                )
                .corrected_by(Edit {
                    start: removed.start,
                    end: removed.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `(block (send (const (const {cbase nil?} :Gem) :Specification) :new) ...)`.
///
/// Unlike the `GemspecHelp` pattern the other cops share, this one puts no condition on the block's
/// parameters -- which is why a specification opened without any reaches the crash above rather than
/// being passed over.
fn is_specification(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(block) = call.field("block") else {
        return false;
    };
    // `on_block` alone: `Gem::Specification.new { _1.date = 1 }` is a `numblock` upstream and reaches
    // no handler.
    if block.field("parameters").is_none() {
        return false;
    }
    if !arguments(call).is_empty() {
        return false;
    }
    if call
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "new")
    {
        return false;
    }
    let Some(receiver) = call.field("receiver") else {
        return false;
    };
    receiver.kind_str() == "scope_resolution"
        && receiver
            .field("name")
            .is_some_and(|name| context.source.node_text(name) == "Specification")
        && receiver
            .field("scope")
            .is_some_and(|scope| top_level_constant(scope, "Gem", context))
}

/// `block_node.first_argument.source`: the text of the block's first parameter, whatever shape it
/// has. A destructured `|(a, b)|` yields `(a, b)`, which no receiver can be written as.
fn block_parameter<'a>(call: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let parameters = call.field("block")?.field("parameters")?;
    let first = named_children_of(parameters, context).first().copied()?;
    Some(context.source.node_text(first))
}

/// The first descendant of the specification block that sets a deprecated attribute, and which one.
///
/// Upstream's `use_deprecated_attributes?` reassigns its own `node` as it walks the attribute list,
/// so a `+=` is only recognised for the *first* attribute: by the second iteration `node` has become
/// the left-hand side of the `op_asgn`, whose method name lacks the `=` the later attributes are
/// looked up by. `spec.test_files += x` is therefore reported while `spec.date += x` is not.
fn first_deprecated<'tree>(
    call: Node<'tree>,
    parameter: &str,
    context: &'tree RuleContext<'_>,
) -> Option<(Node<'tree>, &'static str)> {
    let mut stack = Vec::new();
    push_named_children_in(call, context, &mut stack);
    while let Some(node) = stack.pop() {
        if let Some(attribute) = deprecated_attribute(node, parameter, context) {
            return Some((node, attribute));
        }
        push_named_children_in(node, context, &mut stack);
    }
    None
}

/// Which deprecated attribute `node` sets, replaying upstream's walk over the attribute list.
fn deprecated_attribute(
    node: Node<'_>,
    parameter: &str,
    context: &RuleContext<'_>,
) -> Option<&'static str> {
    let mut current = node;
    for attribute in DEPRECATED {
        // `node_and_method_name`: an `op_asgn` is looked up by its left-hand side under the bare
        // attribute name, anything else by the setter `attribute=`.
        let (target, setter) = match current.kind_str() {
            "operator_assignment" => (current.field("left"), false),
            _ => (Some(current), true),
        };
        let target = target?;
        current = target;
        if sets_attribute(target, parameter, attribute, setter, context) {
            return Some(attribute);
        }
    }
    None
}

/// Whether `node` is the `send` that calls `attribute` (or `attribute=`) on the block parameter.
fn sets_attribute(
    node: Node<'_>,
    parameter: &str,
    attribute: &str,
    setter: bool,
    context: &RuleContext<'_>,
) -> bool {
    // `spec.date = value` is a single setter `send` upstream, written here as an assignment whose
    // target is the call; a target of `spec.a, spec.b = 1, 2` is a setter `send` on its own.
    let call = match node.kind_str() {
        "assignment" => match node.field("left") {
            Some(left) if left.kind_str() == "call" => left,
            _ => return false,
        },
        "call" => node,
        _ => return false,
    };
    // A setter reaches upstream with the `=` in its name only where an assignment put it there.
    let named_as_setter = node.kind_str() == "assignment"
        || node
            .parent()
            .is_some_and(|parent| parent.kind_str() == "left_assignment_list");
    if named_as_setter != setter {
        return false;
    }
    if !is_plain_send(call, context) {
        return false;
    }
    call.field("receiver")
        .is_some_and(|receiver| context.source.node_text(receiver) == parameter)
        && call
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == attribute)
}
