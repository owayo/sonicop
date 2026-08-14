use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, pair_key_symbol};

/// `LITERAL_NODE_TYPES`: the literal each conversion would be handed back unchanged.
fn literal_kinds(method: &str) -> &'static [&'static str] {
    match method {
        // `?a` is a one-character `str` upstream rather than a type of its own.
        "to_s" => &["string", "chained_string", "heredoc_beginning", "character"],
        "to_sym" => &["simple_symbol", "delimited_symbol", "hash_key_symbol"],
        "to_i" => &["integer"],
        "to_f" => &["float"],
        "to_r" => &["rational"],
        "to_c" => &["complex"],
        "to_a" => &["array", "string_array", "symbol_array"],
        "to_h" => &["hash"],
        // `to_set` has no literal at all, and `to_d` is not one of the conversion methods.
        _ => &[],
    }
}

/// `RESTRICT_ON_SEND`.
const RESTRICTED: [&str; 10] = [
    "to_s", "to_sym", "to_i", "to_f", "to_r", "to_c", "to_a", "to_h", "to_set", "to_d",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        if !RESTRICTED.contains(&method) || !arguments(node).is_empty() {
            continue;
        }
        if hash_or_set_with_block(node, method) {
            continue;
        }
        let Some(receiver) = find_receiver(node) else {
            continue;
        };
        let redundant = literal_kinds(method).contains(&receiver.kind_str())
            || is_constructor(method, receiver, context)
            || chained_conversion(receiver, method, context)
            || chained_to_typed_method(receiver, method, context);
        if !redundant {
            continue;
        }
        let Some(dot) = node.field("operator") else {
            continue;
        };
        let end = node
            .field("arguments")
            .map_or_else(|| selector.end_byte(), |list| list.end_byte());
        offenses.push(
            context
                .offense(
                    format!("Redundant `{method}` detected."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: dot.start_byte(),
                    end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `hash_or_set_with_block?`: a block makes `to_h` and `to_set` do something of their own.
fn hash_or_set_with_block(node: Node<'_>, method: &str) -> bool {
    matches!(method, "to_h" | "to_set")
        && (node.field("block").is_some()
            || arguments(node)
                .last()
                .is_some_and(|last| last.first().kind_str() == "block_argument"))
}

/// `find_receiver`: parentheses holding one expression are not part of what was converted.
fn find_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut receiver = node.field("receiver")?;
    while receiver.kind_str() == "parenthesized_statements" {
        let children: Vec<Node<'_>> = named_children(receiver)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect();
        match children.as_slice() {
            [only] => receiver = *only,
            _ => break,
        }
    }
    Some(receiver)
}

/// `constructor?`: the receiver is a call that already built a value of that type.
fn is_constructor(method: &str, receiver: Node<'_>, context: &RuleContext<'_>) -> bool {
    let matched = match method {
        "to_s" => {
            class_call(receiver, "String", &["new"], context)
                || kernel_call(receiver, "String", context)
        }
        "to_i" => kernel_call(receiver, "Integer", context),
        "to_f" => kernel_call(receiver, "Float", context),
        "to_d" => kernel_call(receiver, "BigDecimal", context),
        "to_r" => kernel_call(receiver, "Rational", context),
        "to_c" => kernel_call(receiver, "Complex", context),
        "to_a" => {
            class_call(receiver, "Array", &["new", "[]"], context)
                || kernel_call(receiver, "Array", context)
        }
        "to_h" => {
            class_call(receiver, "Hash", &["new", "[]"], context)
                || kernel_call(receiver, "Hash", context)
        }
        "to_set" => class_call(receiver, "Set", &["new", "[]"], context),
        _ => false,
    };
    // `constructor_suppresses_exceptions?`: `Integer(x, exception: false)` may have answered `nil`.
    matched && !suppresses_exceptions(receiver, context)
}

/// `(send (const {cbase nil?} :Name) {methods} ...)`, including the index form the grammar spells
/// as `element_reference`. A block on `Hash.new` is a `block` upstream, which the pattern allows.
fn class_call(
    receiver: Node<'_>,
    class: &str,
    methods: &[&str],
    context: &RuleContext<'_>,
) -> bool {
    match receiver.kind_str() {
        "element_reference" => {
            methods.contains(&"[]")
                && receiver
                    .field("object")
                    .is_some_and(|object| is_top_level_constant(object, class, context))
        }
        "call" => {
            receiver
                .field("method")
                .is_some_and(|method| methods.contains(&context.source.node_text(method)))
                && receiver
                    .field("receiver")
                    .is_some_and(|inner| is_top_level_constant(inner, class, context))
        }
        _ => false,
    }
}

/// `#type_constructor?`: `Name(...)` written bare or through `Kernel`.
fn kernel_call(receiver: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    receiver.kind_str() == "call"
        && receiver
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == name)
        && receiver
            .field("receiver")
            .is_none_or(|inner| is_top_level_constant(inner, "Kernel", context))
}

fn is_top_level_constant(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == name,
        "scope_resolution" => {
            node.field("scope").is_none()
                && node
                    .field("name")
                    .is_some_and(|inner| context.source.node_text(inner) == name)
        }
        _ => false,
    }
}

fn suppresses_exceptions(receiver: Node<'_>, context: &RuleContext<'_>) -> bool {
    arguments(receiver).iter().any(|argument| {
        argument.parts().iter().any(|part| {
            pair_key_symbol(*part, context) == Some("exception")
                && part
                    .field("value")
                    .is_some_and(|value| value.kind_str() == "false")
        })
    })
}

/// `chained_conversion?`: the same conversion twice over.
fn chained_conversion(receiver: Node<'_>, method: &str, context: &RuleContext<'_>) -> bool {
    receiver.kind_str() == "call"
        && receiver
            .field("method")
            .is_some_and(|name| context.source.node_text(name) == method)
}

/// `TYPED_METHODS`: methods whose answer is already a string.
fn chained_to_typed_method(receiver: Node<'_>, method: &str, context: &RuleContext<'_>) -> bool {
    method == "to_s"
        && receiver.kind_str() == "call"
        && receiver
            .field("method")
            .is_some_and(|name| matches!(context.source.node_text(name), "inspect" | "to_json"))
}
