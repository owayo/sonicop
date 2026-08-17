//! `Style/MapIntoArray`: pushing into an array you just made is what `map` returns.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::variable_force::Variable;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;
use crate::rules::support::final_pos;

use super::conditional::UpstreamParent;

/// `BlockNode::VOID_CONTEXT_METHODS`.
const VOID_CONTEXT_METHODS: &[&str] = &["each", "tap"];

/// The three methods that append one element.
const PUSH_METHODS: &[&str] = &["<<", "push", "append"];

/// Argument kinds `suitable_argument_node?` rejects.
const UNSUITABLE: &[&str] = &[
    "splat_argument",
    "block_argument",
    "forward_argument",
    "hash_splat_argument",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let new_method_name = context
        .setting_of::<std::collections::BTreeMap<String, String>>(
            "Style/CollectionMethods",
            "PreferredMethods",
        )
        .and_then(|preferred| preferred.get("map").cloned())
        .unwrap_or_else(|| "map".to_owned());
    for node in context.nodes_of("call") {
        let Some(push) = each_block_with_push(node, context) else {
            continue;
        };
        let Some(destination) = find_destination(context, push.destination) else {
            continue;
        };
        let variable = &context.variable_analysis().variables[destination];
        let tap =
            empty_array_tap(context, variable.declaration).filter(|tap| tap_body_is(*tap, node));
        let assignment = match tap {
            Some(_) => None,
            None => {
                let Some(assignment) = closest_assignment(variable, node) else {
                    continue;
                };
                if !is_empty_array_assignment(assignment, context)
                    || !used_only_for_mapping(context, node, variable, assignment)
                {
                    continue;
                }
                Some(assignment)
            }
        };
        let Some(selector) = node.field("method") else {
            continue;
        };
        let offense = context.offense(
            format!("Use `{new_method_name}` instead of `each` to map elements into an array."),
            node.byte_range(),
        );
        // `next if return_value_used?(block)`: the array is handed on, so the `each` is not the
        // whole story and nothing can be rewritten.
        if return_value_used(context, node) {
            offenses.push(offense);
            continue;
        }
        let text = context.source.text();
        let mut edits = vec![Edit {
            start: selector.start_byte(),
            end: selector.end_byte(),
            replacement: new_method_name.clone(),
            safe: true,
        }];
        match (tap, assignment) {
            // `remove_tap`: what is left is the block on its own.
            (Some(tap), _) => {
                edits.push(removal(tap.start_byte()..node.start_byte()));
                let Some(close) = super::conditional::token(
                    match tap.field("block") {
                        Some(block) => block,
                        None => continue,
                    },
                    &["}", "end"],
                ) else {
                    continue;
                };
                edits.push(removal(
                    final_pos(text, close.start_byte(), false, false, true, false)
                        ..close.end_byte(),
                ));
            }
            // `remove_assignment`: the blank after it goes too, line break included.
            (None, Some(assignment)) => {
                let end = final_pos(text, assignment.end_byte(), true, false, true, false);
                edits.push(removal(
                    assignment.start_byte()..final_pos(text, end, true, false, false, false),
                ));
            }
            (None, None) => continue,
        }
        // `correct_push_node`: only the value pushed survives.
        if push.argument_is_braceless_hash {
            edits.push(Edit {
                start: push.argument.start,
                end: push.argument.start,
                replacement: "{ ".to_owned(),
                safe: true,
            });
            edits.push(Edit {
                start: push.argument.end,
                end: push.argument.end,
                replacement: " }".to_owned(),
                safe: true,
            });
        }
        edits.push(removal(push.range.start..push.argument.start));
        edits.push(removal(push.argument.end..push.range.end));
        // `correct_return_value_handling`.
        if let Some(sibling) = right_sibling(node)
            && sibling.kind_str() == "identifier"
            && context.source.node_text(sibling) == variable.name
        {
            edits.push(removal(
                final_pos(text, sibling.start_byte(), false, false, true, false)
                    ..sibling.end_byte(),
            ));
        }
        edits.push(Edit {
            start: node.start_byte(),
            end: node.start_byte(),
            replacement: format!("{} = ", variable.name),
            safe: true,
        });
        offenses.push(offense.corrected_by_all(edits));
    }
}

fn removal(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// What the block pushes and where.
struct Push<'tree> {
    /// The `dest` the push is written on.
    destination: Node<'tree>,
    /// The whole `dest << value` expression.
    range: Range<usize>,
    argument: Range<usize>,
    argument_is_braceless_hash: bool,
}

/// `each_block_with_push?`.
fn each_block_with_push<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Push<'tree>> {
    if context.source.node_text(node.field("method")?) != "each" {
        return None;
    }
    // `!{nil? self}`: a receiverless `each` and `self.each` are not what the cop is about.
    let receiver = node.field("receiver")?;
    if receiver.kind_str() == "self" {
        return None;
    }
    // `(send !{nil? self} :each)` names no argument, so a pattern with no argument slots only
    // matches a call that has none. `StringIO#each(sep)` and friends take one and mean something
    // else, and there is no `map` that would say the same thing.
    if !arguments(node).is_empty() {
        return None;
    }
    let block = node.field("block")?;
    // `^({begin kwbegin block} ...)`: the block has to stand among statements, not be the whole
    // body of a definition.
    match super::conditional::upstream_parent(node)? {
        UpstreamParent::Begin(_) => {}
        UpstreamParent::Node(parent)
            if matches!(parent.kind_str(), "block" | "do_block" | "begin") => {}
        UpstreamParent::Node(_) => return None,
    }
    let body = super::nodes::children(block.field("body")?);
    let [statement] = body.as_slice() else {
        return None;
    };
    push_of(*statement, context)
}

/// `(send (lvar _) {:<< :push :append} #suitable_argument_node?)`, which `<<` writes as an operator.
fn push_of<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Push<'tree>> {
    let (destination, argument) = match node.kind_str() {
        "binary" => {
            if context.source.node_text(node.field("operator")?) != "<<" {
                return None;
            }
            (node.field("left")?, node.field("right")?.byte_range())
        }
        "call" => {
            if !PUSH_METHODS.contains(&context.source.node_text(node.field("method")?))
                || node.field("block").is_some()
            {
                return None;
            }
            let list = arguments(node);
            let [only] = list.as_slice() else {
                return None;
            };
            if UNSUITABLE.contains(&only.first().kind_str()) {
                return None;
            }
            (node.field("receiver")?, only.range())
        }
        _ => return None,
    };
    if destination.kind_str() != "identifier" {
        return None;
    }
    let first = context
        .nodes()
        .find(|candidate| candidate.byte_range() == argument)
        .map(|candidate| candidate.kind_str())
        .unwrap_or_default();
    Some(Push {
        destination,
        range: node.byte_range(),
        argument,
        argument_is_braceless_hash: first == "pair",
    })
}

/// `find_dest_var`: the variable this very read belongs to.
fn find_destination(context: &RuleContext<'_>, read: Node<'_>) -> Option<usize> {
    context
        .variable_analysis()
        .variables
        .iter()
        .position(|variable| {
            variable
                .references
                .iter()
                .any(|reference| reference.node.id() == read.id())
        })
}

/// `find_closest_assignment`: the last write that finished before the block began.
fn closest_assignment<'tree>(variable: &Variable<'tree>, block: Node<'_>) -> Option<Node<'tree>> {
    variable
        .assignments
        .iter()
        .rev()
        .map(|assignment| assignment.node)
        .find(|node| node.end_byte() < block.start_byte())
}

/// `empty_array_asgn?`.
fn is_empty_array_assignment(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "assignment" {
        return false;
    }
    let Some(value) = node.field("right") else {
        return false;
    };
    let empty_array = |node: Node<'_>| {
        matches!(node.kind_str(), "array" | "string_array" | "symbol_array")
            && super::nodes::children(node).is_empty()
    };
    if empty_array(value) {
        return true;
    }
    if value.kind_str() == "element_reference" {
        return super::nodes::children(value)
            .first()
            .is_some_and(|object| super::nodes::is_top_level_constant(*object, "Array", context))
            && super::nodes::children(value).len() == 1;
    }
    if value.kind_str() != "call" || value.field("block").is_some() {
        return false;
    }
    let Some(name) = value.field("method") else {
        return false;
    };
    let list = arguments(value);
    match context.source.node_text(name) {
        // `Array.new` and `Array.new([])`.
        "new" => {
            value.field("receiver").is_some_and(|receiver| {
                super::nodes::is_top_level_constant(receiver, "Array", context)
            }) && match list.as_slice() {
                [] => true,
                [only] => empty_array(only.first()),
                _ => false,
            }
        }
        // `Array([])`.
        "Array" => {
            value.field("receiver").is_none()
                && matches!(list.as_slice(), [only] if empty_array(only.first()))
        }
        _ => false,
    }
}

/// `dest_used_only_for_mapping?`.
fn used_only_for_mapping(
    context: &RuleContext<'_>,
    block: Node<'_>,
    variable: &Variable<'_>,
    assignment: Node<'_>,
) -> bool {
    let (Some(left), Some(right)) = (context.parent(assignment), context.parent(block)) else {
        return false;
    };
    if left.id() != right.id() {
        return false;
    }
    let range = assignment.start_byte()..block.end_byte();
    let inside = |node: Node<'_>| range.start <= node.start_byte() && node.end_byte() <= range.end;
    variable
        .references
        .iter()
        .filter(|reference| inside(reference.node))
        .count()
        == 1
        && variable
            .assignments
            .iter()
            .filter(|other| inside(other.node))
            .count()
            == 1
}

/// `empty_array_tap`: `[].tap { |dest| ... }`, reached from the parameter it declares.
fn empty_array_tap<'tree>(
    context: &'tree RuleContext<'_>,
    declaration: Node<'tree>,
) -> Option<Node<'tree>> {
    let parameters = context.parent(declaration)?;
    if parameters.kind_str() != "block_parameters" || super::nodes::children(parameters).len() != 1
    {
        return None;
    }
    let block = context.parent(parameters)?;
    let call = context.parent(block)?;
    if call.kind_str() != "call" || context.source.node_text(call.field("method")?) != "tap" {
        return None;
    }
    let receiver = call.field("receiver")?;
    (receiver.kind_str() == "array" && super::nodes::children(receiver).is_empty()).then_some(call)
}

/// `tap_block_node.body == node`.
fn tap_body_is(tap: Node<'_>, node: Node<'_>) -> bool {
    tap.field("block")
        .and_then(|block| block.field("body"))
        .is_some_and(|body| {
            matches!(super::nodes::children(body).as_slice(), [only] if only.id() == node.id())
        })
}

/// `return_value_used?`.
fn return_value_used(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = super::conditional::upstream_parent(node) else {
        return false;
    };
    match parent {
        UpstreamParent::Begin(container) => {
            super::conditional::self_statements(container)
                .last()
                .is_some_and(|last| last.id() == node.id())
                && return_value_used(context, container)
        }
        // `begin ... end` is a `kwbegin` upstream, which holds its statements itself rather than
        // wrapping them in a `begin` -- so it is not one of the statement containers, and only its
        // last statement carries its value.
        UpstreamParent::Node(parent) if parent.kind_str() == "begin" => {
            super::conditional::self_statements(parent)
                .last()
                .is_some_and(|last| last.id() == node.id())
                && return_value_used(context, parent)
        }
        UpstreamParent::Node(parent) => match parent.kind_str() {
            "block" | "do_block" => context
                .parent(parent)
                .and_then(|call| call.field("method"))
                .is_none_or(|name| !VOID_CONTEXT_METHODS.contains(&context.source.node_text(name))),
            "method" | "singleton_method" => parent.field("name").is_none_or(|name| {
                let name = context.source.node_text(name);
                !(name == "initialize" || name.ends_with('='))
            }),
            "ensure" | "for" => false,
            _ => true,
        },
    }
}

/// `node.right_sibling` among the statements it stands with.
fn right_sibling<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let container = node.parent()?;
    let statements = super::conditional::self_statements(container);
    let position = statements
        .iter()
        .position(|statement| statement.id() == node.id())?;
    statements.get(position + 1).copied()
}
