use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, named_children, send_range};

use super::locals::LocalVariables;
use super::node_equality::identical;

const MSG: &str = "Self-assignment detected.";

/// `COMPARISON_OPERATORS`: the method names ending in `=` that `assignment_method?` refuses.
const COMPARISON_METHODS: [&str; 5] = ["==", "===", "!=", "<=", ">="];

/// The variable kinds `ASSIGNMENT_TYPE_TO_RHS_TYPE` maps. A constant or a call is deliberately
/// missing: upstream's table answers `nil` for those, and nothing equals `nil`.
const VARIABLE_KINDS: [&str; 4] = [
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["assignment", "operator_assignment", "call"]) {
        let range = match node.kind() {
            "call" => {
                if !call_assignment(node, context, &locals) {
                    continue;
                }
                send_range(node, context)
            }
            "assignment" => {
                if !assignment(node, context, &locals) {
                    continue;
                }
                node.byte_range()
            }
            _ => {
                if !operator_assignment(node, context, &locals) {
                    continue;
                }
                node.byte_range()
            }
        };
        offenses.push(context.offense(MSG, range));
    }
}

/// `on_lvasgn` and its aliases, `on_casgn`, `on_masgn`, and the two `on_send` branches for the
/// assignments written with brackets or a dotted setter.
fn assignment(node: Node<'_>, context: &RuleContext<'_>, locals: &LocalVariables<'_>) -> bool {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return false;
    };
    match left.kind() {
        // The assignment itself declares the name, so the same name on its right always reads the
        // variable rather than calling a method.
        kind if VARIABLE_KINDS.contains(&kind) => {
            right.kind() == kind
                && context.source.node_text(right) == context.source.node_text(left)
        }
        "constant" | "scope_resolution" => constant_self_assignment(left, right, context),
        "left_assignment_list" => multiple_self_assignment(left, right, context),
        "element_reference" => key_assignment(
            left.child_by_field_name("object"),
            &index_arguments(left),
            right,
            context,
            locals,
        ),
        "call" => {
            let Some(method) = left.child_by_field_name("method") else {
                return false;
            };
            attribute_assignment(
                left.child_by_field_name("receiver"),
                context.source.node_text(method),
                right,
                context,
            )
        }
        _ => false,
    }
}

/// The `on_send` half of the cop, for the setters written as an ordinary call: `obj.attr=(value)`
/// and `hash.[]=(key, value)`.
fn call_assignment(node: Node<'_>, context: &RuleContext<'_>, locals: &LocalVariables<'_>) -> bool {
    let Some(method) = node.child_by_field_name("method") else {
        return false;
    };
    let name = context.source.node_text(method);
    let call_arguments: Vec<Vec<Node<'_>>> = arguments(node)
        .iter()
        .map(|argument| argument.parts().to_vec())
        .collect();
    let receiver = node.child_by_field_name("receiver");
    if name == "[]=" {
        let Some((value, keys)) = call_arguments.split_last() else {
            return false;
        };
        let [value] = value.as_slice() else {
            return false;
        };
        return key_assignment(receiver, keys, *value, context, locals);
    }
    if !name.ends_with('=') || COMPARISON_METHODS.contains(&name) {
        return false;
    }
    let [argument] = call_arguments.as_slice() else {
        return false;
    };
    let [argument] = argument.as_slice() else {
        return false;
    };
    attribute_assignment(receiver, &name[..name.len() - 1], *argument, context)
}

/// `on_or_asgn` and `on_and_asgn`. Upstream reaches these through node types of their own, so the
/// arithmetic forms -- `x += x` -- are no part of the cop.
fn operator_assignment(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
) -> bool {
    let (Some(left), Some(right), Some(operator)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
        node.child_by_field_name("operator"),
    ) else {
        return false;
    };
    if !matches!(context.source.node_text(operator), "||=" | "&&=") {
        return false;
    }
    match left.kind() {
        kind if VARIABLE_KINDS.contains(&kind) => {
            right.kind() == kind
                && context.source.node_text(right) == context.source.node_text(left)
        }
        "constant" | "scope_resolution" => constant_self_assignment(left, right, context),
        // `reader_self_assignment?`: the left of an `||=` is the *reader*, so the two sides are two
        // calls of the same method rather than a setter and a getter.
        "element_reference" => {
            let Some(reader) = reader_call(right, context) else {
                return false;
            };
            let keys = index_arguments(left);
            reader.method == "[]"
                && receivers_match(left.child_by_field_name("object"), reader.receiver, context)
                && identical_arguments(&keys, &reader.arguments, context)
                && keys.iter().all(|key| !is_call(key, locals, context))
        }
        "call" => {
            let (Some(method), Some(reader)) = (
                left.child_by_field_name("method"),
                reader_call(right, context),
            ) else {
                return false;
            };
            let own = arguments(left)
                .iter()
                .map(|argument| argument.parts().to_vec())
                .collect::<Vec<_>>();
            // `rhs.type == lhs.type`: a `&.` reader never matches a `.` one.
            reader.safe_navigation == is_safe_navigation(left, context)
                && reader.method == context.source.node_text(method)
                && receivers_match(
                    left.child_by_field_name("receiver"),
                    reader.receiver,
                    context,
                )
                && identical_arguments(&own, &reader.arguments, context)
                && own
                    .iter()
                    .all(|argument| !is_call(argument, locals, context))
        }
        _ => false,
    }
}

/// `on_casgn`, and the `casgn` branch of `or_and_asgn_self_assignment?`: the two constants agree on
/// both the namespace they are reached through and the name itself.
fn constant_self_assignment(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (Some(left), Some(right)) = (
        constant_parts(left, context),
        constant_parts(right, context),
    ) else {
        return false;
    };
    left.1 == right.1
        && match (left.0, right.0) {
            (Namespace::None, Namespace::None) | (Namespace::Base, Namespace::Base) => true,
            (Namespace::Node(left), Namespace::Node(right)) => identical(left, right, context),
            _ => false,
        }
}

/// `multiple_self_assignment?`: every name on the left is read back in the same position.
fn multiple_self_assignment(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `rhs.array_type?`. `a, b = c` hands over one value rather than a list, and `a, b = *c` builds
    // an array holding a `splat`, which is no variable read.
    let values = match right.kind() {
        "right_assignment_list" | "array" => named_children(right),
        _ => return false,
    };
    let targets = named_children(left);
    targets.len() == values.len()
        && targets.iter().zip(&values).all(|(target, value)| {
            VARIABLE_KINDS.contains(&target.kind())
                && value.kind() == target.kind()
                && context.source.node_text(*value) == context.source.node_text(*target)
        })
}

/// `handle_key_assignment`: `hash[key] = hash[key]`, where the key is not itself a method call --
/// two calls of the same method may answer differently.
fn key_assignment(
    receiver: Option<Node<'_>>,
    keys: &[Vec<Node<'_>>],
    value: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
) -> bool {
    let Some(reader) = reader_call(value, context) else {
        return false;
    };
    reader.method == "[]"
        && receivers_match(receiver, reader.receiver, context)
        && keys.iter().all(|key| !is_call(key, locals, context))
        && identical_arguments(keys, &reader.arguments, context)
}

/// `handle_attribute_assignment`: `obj.attr = obj.attr`, where the reader takes no arguments.
fn attribute_assignment(
    receiver: Option<Node<'_>>,
    name: &str,
    value: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let Some(reader) = reader_call(value, context) else {
        return false;
    };
    reader.arguments.is_empty()
        && reader.method == name
        && receivers_match(receiver, reader.receiver, context)
}

/// A call read as upstream's `SendNode` presents it. `hash[key]` is a `[]` call there, so the two
/// spellings have to arrive here the same way.
struct Reader<'tree> {
    receiver: Option<Node<'tree>>,
    method: String,
    arguments: Vec<Vec<Node<'tree>>>,
    safe_navigation: bool,
}

fn reader_call<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Reader<'tree>> {
    match node.kind() {
        "element_reference" => Some(Reader {
            receiver: node.child_by_field_name("object"),
            method: "[]".to_owned(),
            arguments: index_arguments(node),
            safe_navigation: false,
        }),
        "call" => {
            // A call with a block is a `block` node upstream, which answers no method name at all.
            if node.child_by_field_name("block").is_some() {
                return None;
            }
            let method = node.child_by_field_name("method")?;
            Some(Reader {
                receiver: node.child_by_field_name("receiver"),
                method: context.source.node_text(method).to_owned(),
                arguments: arguments(node)
                    .iter()
                    .map(|argument| argument.parts().to_vec())
                    .collect(),
                safe_navigation: is_safe_navigation(node, context),
            })
        }
        _ => None,
    }
}

fn is_safe_navigation(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    call.child_by_field_name("operator")
        .is_some_and(|operator| context.source.node_text(operator) == "&.")
}

/// The arguments between an `element_reference`'s brackets.
fn index_arguments<'tree>(node: Node<'tree>) -> Vec<Vec<Node<'tree>>> {
    let object = node.child_by_field_name("object");
    let mut keys: Vec<Vec<Node<'tree>>> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for child in named_children(node) {
        if object.is_some_and(|object| object.id() == child.id()) || child.kind() == "comment" {
            continue;
        }
        if matches!(child.kind(), "pair" | "hash_splat_argument") {
            hash.push(child);
            continue;
        }
        if !hash.is_empty() {
            keys.push(std::mem::take(&mut hash));
        }
        keys.push(vec![child]);
    }
    if !hash.is_empty() {
        keys.push(hash);
    }
    keys
}

fn receivers_match(
    left: Option<Node<'_>>,
    right: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => identical(left, right, context),
        _ => false,
    }
}

fn identical_arguments(
    left: &[Vec<Node<'_>>],
    right: &[Vec<Node<'_>>],
    context: &RuleContext<'_>,
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| identical(*left, *right, context))
        })
}

/// `call_type?`: whether upstream's parser would have built a `send` or a `csend` here. A key that
/// calls a method may answer differently the second time, which is what the guard is for.
fn is_call(argument: &[Node<'_>], locals: &LocalVariables<'_>, context: &RuleContext<'_>) -> bool {
    let [node] = argument else {
        // A brace-less hash is a `hash` upstream, never a call.
        return false;
    };
    match node.kind() {
        "call" | "element_reference" => true,
        "identifier" => !locals.is_lvar(*node),
        // An operator is a method: `a + b` is `(send a :+ b)`. The logical operators are not.
        "binary" => node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                !matches!(
                    context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                )
            }),
        // The parser folds the sign of a numeric literal into the literal; every other unary is a
        // call of the operator method.
        "unary" => {
            let operator = node
                .child_by_field_name("operator")
                .map(|operator| context.source.node_text(operator));
            let numeric = node
                .child_by_field_name("operand")
                .is_some_and(|operand| matches!(operand.kind(), "integer" | "float"));
            match operator {
                Some("-" | "+") => !numeric,
                Some("defined?" | "not") => false,
                Some(_) => true,
                None => false,
            }
        }
        _ => false,
    }
}

/// How a constant is reached, which is what upstream's `namespace` answers. A plain `Foo` has none,
/// `::Foo` is reached from the top level, and `A::Foo` through the node naming `A`.
enum Namespace<'tree> {
    None,
    Base,
    Node(Node<'tree>),
}

fn constant_parts<'a, 'tree>(
    node: Node<'tree>,
    context: &'a RuleContext<'_>,
) -> Option<(Namespace<'tree>, &'a str)> {
    match node.kind() {
        "constant" => Some((Namespace::None, context.source.node_text(node))),
        "scope_resolution" => {
            let name = node.child_by_field_name("name")?;
            if name.kind() != "constant" {
                return None;
            }
            let namespace = node
                .child_by_field_name("scope")
                .map_or(Namespace::Base, Namespace::Node);
            Some((namespace, context.source.node_text(name)))
        }
        _ => None,
    }
}
