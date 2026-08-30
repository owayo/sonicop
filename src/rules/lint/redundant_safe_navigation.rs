use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, string_text, symbol_name};

use super::blocks::BLOCK_KINDS;
use super::literals::{is_literal, literal_type};
use super::nil_methods::NIL_METHODS;
use super::nil_receiver::cant_be_nil;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

const MSG: &str = "Redundant safe navigation detected, use `.` instead.";
const MSG_LITERAL: &str = "Redundant safe navigation with default literal detected.";
const MSG_NON_NIL: &str = "Redundant safe navigation on non-nil receiver (detected by analyzing \
                           previous code/method invocations).";

/// `GUARANTEED_INSTANCE_METHODS`: conversions that cannot answer `nil`.
const GUARANTEED_INSTANCE_METHODS: &[&str] = &["to_s", "to_i", "to_f", "to_a", "to_h"];

/// The conditional and loop kinds `conditional?` and `post_condition_loop?` cover.
const CONDITIONALS: &[&str] = &[
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "case",
    "case_match",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = context
        .setting::<Vec<String>>("AllowedMethods")
        .unwrap_or_default();
    // `InferNonNilReceiver`: look into the code before the call rather than at the call alone.
    let infer_non_nil = context
        .setting::<bool>("InferNonNilReceiver")
        .unwrap_or(false);
    let additional_nil_methods = context
        .setting::<Vec<String>>("AdditionalNilMethods")
        .unwrap_or_default();

    for node in context.nodes_of("call") {
        let Some(dot) = safe_navigation_dot(node, context) else {
            continue;
        };
        if infer_non_nil
            && let Some(receiver) = node.field("receiver")
            && cant_be_nil(context, receiver, &additional_nil_methods)
        {
            offenses.push(
                context
                    .offense(MSG_NON_NIL, dot.clone())
                    .corrected_by(Edit {
                        start: dot.start,
                        end: dot.end,
                        replacement: ".".to_owned(),
                        safe: false,
                    }),
            );
            continue;
        }
        if guarded_by_nil_receiver(node, &allowed, context) {
            continue;
        }
        offenses.push(context.offense(MSG, dot.clone()).corrected_by(Edit {
            start: dot.start,
            end: dot.end,
            replacement: ".".to_owned(),
            safe: false,
        }));
    }
    for node in context.nodes_of("binary") {
        check_conversion_with_default(node, context, offenses);
    }
}

fn safe_navigation_dot(
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<std::ops::Range<usize>> {
    let operator = node.field("operator")?;
    (context.source.node_text(operator) == "&.").then(|| operator.byte_range())
}

/// `guarded_by_nil_receiver?`.
fn guarded_by_nil_receiver(node: Node<'_>, allowed: &[String], context: &RuleContext<'_>) -> bool {
    let Some(receiver) = node.field("receiver") else {
        return true;
    };
    if assume_receiver_instance_exists(receiver, context) {
        return false;
    }
    let guaranteed = guaranteed_instance(receiver, context);
    if !guaranteed && !checks_nil(node, allowed, context) {
        return true;
    }
    responds_to_nil_method(node, context) && !guaranteed
}

/// `assume_receiver_instance_exists?`: a class name, `self`, or a literal other than `nil`.
fn assume_receiver_instance_exists(receiver: Node<'_>, context: &RuleContext<'_>) -> bool {
    if let Some(name) = constant_short_name(receiver, context)
        && !name
            .chars()
            .all(|letter| letter.is_ascii_digit() || letter.is_ascii_uppercase() || letter == '_')
    {
        return true;
    }
    receiver.kind_str() == "self"
        || (is_literal(receiver, context) && literal_type(receiver, context) != Some("nil"))
}

/// `receiver.short_name` for a `const`, which is the last segment of a scoped name.
fn constant_short_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" => Some(context.source.node_text(node.field("name")?)),
        _ => None,
    }
}

/// `guaranteed_instance?`: a plain send of a conversion method, possibly carrying a block.
fn guaranteed_instance(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let receiver = if BLOCK_KINDS.contains(&node.kind_str()) {
        return false;
    } else {
        node
    };
    // A block written after the call is part of the call here, and `send_node` upstream.
    if receiver.kind_str() != "call" || safe_navigation_dot(receiver, context).is_some() {
        return false;
    }
    receiver.field("method").is_some_and(|method| {
        GUARANTEED_INSTANCE_METHODS.contains(&context.source.node_text(method))
    })
}

/// `check?`: an allowed predicate written where its result decides a branch.
fn checks_nil(node: Node<'_>, allowed: &[String], context: &RuleContext<'_>) -> bool {
    let Some(method) = node.field("method") else {
        return false;
    };
    if !allowed
        .iter()
        .any(|name| name == context.source.node_text(method))
    {
        return false;
    }
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    if CONDITIONALS.contains(&parent.kind_str())
        && parent
            .field("condition")
            .is_some_and(|condition| condition.id() == node.id())
    {
        return true;
    }
    match parent.kind_str() {
        "binary" => parent.field("operator").is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "&&" | "and" | "||" | "or"
            )
        }),
        "unary" => parent
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "!"),
        _ => false,
    }
}

/// `(csend _ :respond_to? (sym %NIL_METHODS))`.
fn responds_to_nil_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "respond_to?")
    {
        return false;
    }
    let given = arguments(node);
    given.len() == 1
        && symbol_name(given[0].first(), context).is_some_and(|name| NIL_METHODS.contains(&name))
}

/// `on_or`: `foo&.to_h || {}` and its four siblings, where the default repeats what `nil` gives.
fn check_conversion_with_default(
    node: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    if node
        .field("operator")
        .is_none_or(|operator| !matches!(context.source.node_text(operator), "||" | "or"))
    {
        return;
    }
    let (Some(left), Some(right)) = (node.field("left"), node.field("right")) else {
        return;
    };
    // `foo&.to_h { ... } || {}` reaches the call through the block upstream wraps it in.
    let call = left;
    let Some(dot) = safe_navigation_dot(call, context) else {
        return;
    };
    let Some(method) = call.field("method") else {
        return;
    };
    let has_block = call
        .field("block")
        .is_some_and(|block| BLOCK_KINDS.contains(&block.kind_str()));
    let matched = match context.source.node_text(method) {
        "to_h" => right.kind_str() == "hash" && named_children_of(right, context).is_empty(),
        "to_a" if !has_block => right.kind_str() == "array" && named_children_of(right, context).is_empty(),
        "to_i" if !has_block => {
            right.kind_str() == "integer" && context.source.node_text(right) == "0"
        }
        "to_f" if !has_block => {
            right.kind_str() == "float" && context.source.node_text(right) == "0.0"
        }
        "to_s" if !has_block => {
            literal_type(right, context) == Some("str") && string_text(right, context).is_empty()
        }
        _ => false,
    };
    if !matched {
        return;
    }
    let range = dot.start..node.end_byte();
    offenses.push(context.offense(MSG_LITERAL, range).corrected_by_all([
        Edit {
            start: dot.start,
            end: dot.end,
            replacement: ".".to_owned(),
            safe: false,
        },
        Edit {
            start: left.end_byte(),
            end: node.end_byte(),
            replacement: String::new(),
            safe: false,
        },
    ]));
}
