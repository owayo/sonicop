use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children};

use super::node_equality::identical;
use crate::rules::send_node::named_children_of;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((class_name, elements)) = set_init_elements(node, context) else {
            continue;
        };
        let mut seen: Vec<Node<'_>> = Vec::new();
        for (index, &element) in elements.iter().enumerate() {
            // Only a value the cop can compare: anything computed may differ at run time.
            if !is_comparable(element, context) {
                continue;
            }
            if !seen.iter().any(|other| identical(*other, element, context)) {
                seen.push(element);
                continue;
            }
            let previous = elements[index - 1];
            let message = format!("Remove the duplicate element in {class_name}.");
            offenses.push(
                context
                    .offense(message, element.byte_range())
                    .corrected_by(Edit {
                        start: previous.end_byte(),
                        end: element.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
    }
}

/// `set_init_elements`, paired with the name the message calls the class by.
fn set_init_elements<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<(String, Vec<Node<'tree>>)> {
    if node.kind_str() == "element_reference" {
        let object = node.field("object")?;
        let name = set_class_name(object, context)?;
        let elements = named_children_of(node, context)
            .into_iter()
            .filter(|child| child.kind_str() != "comment" && child.id() != object.id())
            .collect();
        return Some((name, elements));
    }
    let method = node.field("method")?;
    match context.source.node_text(method) {
        "new" => {
            let receiver = node.field("receiver")?;
            let name = set_class_name(receiver, context)?;
            let call_arguments = arguments(node);
            let [only] = call_arguments.as_slice() else {
                return None;
            };
            let array = only.first();
            // `%i[…]` and `%w[…]` are `array` nodes upstream; the grammar names them by their
            // percent spelling, so `Set.new(%i[foo bar foo])` fell out of the arm entirely.
            matches!(array.kind_str(), "array" | "symbol_array" | "string_array")
                .then(|| (name, literal_elements(array)))
        }
        // `(call (array $...) :to_set)`: the class is not written, so the message names `Set`.
        "to_set" => {
            let receiver = node.field("receiver")?;
            (receiver.kind_str() == "array")
                .then(|| ("Set".to_owned(), literal_elements(receiver)))
        }
        _ => None,
    }
}

fn literal_elements<'tree>(array: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(array)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

/// `(const {nil? cbase} {:Set :SortedSet})`, and the `const_name` the message uses -- which drops
/// the leading `::` of a constant reached from the top level.
fn set_class_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let name = match node.kind_str() {
        "constant" => context.source.node_text(node),
        "scope_resolution" if node.field("scope").is_none() => {
            context.source.node_text(node.field("name")?)
        }
        _ => return None,
    };
    matches!(name, "Set" | "SortedSet").then(|| name.to_owned())
}

/// `literal? || const_type? || variable?`: what the cop is willing to call the same twice.
fn is_comparable(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `variable?` is `lvar`/`ivar`/`cvar`/`gvar`. A bare name that was never assigned is a `send`
    // upstream -- `Set[foo, foo]` calls `foo` twice and may well get two different values.
    if node.kind_str() == "identifier" && !context.variable_analysis().is_variable_reference(node) {
        return false;
    }
    matches!(
        node.kind_str(),
        "integer"
            | "float"
            | "rational"
            | "complex"
            | "string"
            | "simple_symbol"
            | "delimited_symbol"
            // The elements of a `%i[…]` / `%w[…]`, which upstream spells as plain `sym` and `str`.
            | "bare_symbol"
            | "bare_string"
            | "hash_key_symbol"
            | "character"
            | "true"
            | "false"
            | "nil"
            | "regex"
            | "array"
            | "hash"
            | "range"
            | "string_array"
            | "symbol_array"
            | "chained_string"
            | "constant"
            | "scope_resolution"
            | "instance_variable"
            | "class_variable"
            | "global_variable"
            // `variable?` covers a local variable, which the grammar spells as a bare identifier.
            | "identifier"
            | "self"
    )
}
