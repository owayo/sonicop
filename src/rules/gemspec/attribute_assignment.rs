use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{is_plain_send, named_children};

use super::support::{first_specification_variable, is_literal, is_specification_receiver};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let variable = first_specification_variable(context);
    // `source_assignments`: every `spec.attribute = value`, keyed by the attribute without the `=`
    // upstream's method name carries.
    let mut assigned: Vec<&str> = Vec::new();
    // `source_indexed_assignments`: every `spec.attribute[key] = value`, keyed by the attribute the
    // index was taken on.
    let mut indexed: Vec<(&str, Node<'_>)> = Vec::new();
    for node in context.nodes_of("assignment") {
        let Some(left) = node.field("left") else {
            continue;
        };
        match left.kind_str() {
            "call" => {
                if let Some(attribute) = assigned_attribute(left, variable, context) {
                    assigned.push(attribute);
                }
            }
            "element_reference" => {
                if let Some(attribute) = indexed_attribute(left, variable, context) {
                    indexed.push((attribute, node));
                }
            }
            // `spec.a, spec.b = 1, 2` names one attribute per target. An indexed target of a
            // multiple assignment carries no value of its own, so it never matches upstream's
            // `(send _ :[]= literal? _)`, which insists on both an index and a value.
            "left_assignment_list" => {
                for target in named_children(left) {
                    if target.kind_str() == "call"
                        && let Some(attribute) = assigned_attribute(target, variable, context)
                    {
                        assigned.push(attribute);
                    }
                }
            }
            _ => {}
        }
    }

    for (attribute, node) in indexed {
        if assigned.contains(&attribute) {
            offenses.push(context.offense(
                "Use consistent style for Gemspec attributes assignment.",
                node.byte_range(),
            ));
        }
    }
}

/// The attribute `spec.attribute = value` sets, taken from the target of the assignment.
///
/// Upstream reaches this through `assignment_method_declarations` filtered by `assignment_method?`:
/// a `send` on the specification whose name ends in `=`, which is what a setter reaches a cop as.
/// A comparison is excluded there, and none of those can stand as an assignment target here.
fn assigned_attribute<'a>(
    target: Node<'_>,
    variable: Option<&str>,
    context: &'a RuleContext<'_>,
) -> Option<&'a str> {
    let receiver = target.field("receiver")?;
    if !is_specification_receiver(receiver, variable, context) || !is_plain_send(target, context) {
        return None;
    }
    Some(context.source.node_text(target.field("method")?))
}

/// The attribute `spec.attribute[key] = value` indexes into.
///
/// `(send (send (lvar {spec :_1 :it}) _) :[]= literal? _)`: the attribute is read with no arguments
/// of its own, and the index is a single literal.
fn indexed_attribute<'a>(
    target: Node<'_>,
    variable: Option<&str>,
    context: &'a RuleContext<'_>,
) -> Option<&'a str> {
    let object = target.field("object")?;
    if object.kind_str() != "call" || !is_plain_send(object, context) {
        return None;
    }
    if object.field("arguments").is_some() {
        return None;
    }
    let receiver = object.field("receiver")?;
    if !is_specification_receiver(receiver, variable, context) {
        return None;
    }
    let indices = named_children(target);
    let [index] = indices.get(1..)? else {
        return None;
    };
    if !is_literal(*index) {
        return None;
    }
    Some(context.source.node_text(object.field("method")?))
}
