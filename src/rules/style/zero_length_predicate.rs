//! `Style/ZeroLengthPredicate`: `empty?` says what comparing a length to zero means.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        check_predicate(context, node, offenses);
    }
    for node in context.nodes_of("binary") {
        check_comparison(context, node, offenses);
    }
}

/// `(call (call (...) {:length :size}) :zero?)`.
fn check_predicate(context: &RuleContext<'_>, node: Node<'_>, offenses: &mut Vec<Offense>) {
    if selector_of(context, node) != Some("zero?") || !arguments(node).is_empty() {
        return;
    }
    let Some(receiver) = node.field("receiver") else {
        return;
    };
    let Some(length) = length_call(context, receiver) else {
        return;
    };
    if non_polymorphic_collection(context, length) {
        return;
    }
    let Some(selector) = length.field("method") else {
        return;
    };
    let range = selector.start_byte()..node.end_byte();
    let current = context.source.slice(range.clone());
    // The reported range starts at the `size` selector, so `empty?` stands in for the whole tail;
    // the receiver and the dot in front of it are already outside it.
    offenses.push(
        context
            .offense(
                format!("Use `empty?` instead of `{current}`."),
                range.clone(),
            )
            .corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: "empty?".to_owned(),
                safe: true,
            }),
    );
}

/// The four zero comparisons and the two non-zero ones.
fn check_comparison(context: &RuleContext<'_>, node: Node<'_>, offenses: &mut Vec<Offense>) {
    let (Some(operator), Some(left), Some(right)) = (
        node.field("operator"),
        node.field("left"),
        node.field("right"),
    ) else {
        return;
    };
    let operator = context.source.node_text(operator);
    let left_length = length_call(context, left);
    let right_length = length_call(context, right);
    let left_int = integer_value(context, left);
    let right_int = integer_value(context, right);

    // `zero_length_comparison` / `nonzero_length_comparison`, in the order the patterns are tried.
    // The message quotes the *method name* rather than the receiver: `size == 0`, `0 == size`.
    let (zero, length, literal, on_the_left) =
        match (operator, left_length, right_length, left_int, right_int) {
            ("==", Some(length), _, _, Some(0)) => (true, length, "0", true),
            ("==", _, Some(length), Some(0), _) => (true, length, "0", false),
            ("<", Some(length), _, _, Some(1)) => (true, length, "1", true),
            (">", _, Some(length), Some(1), _) => (true, length, "1", false),
            (">" | "!=", Some(length), _, _, Some(0)) => (false, length, "0", true),
            ("<" | "!=", _, Some(length), Some(0), _) => (false, length, "0", false),
            _ => return,
        };
    let name = selector_of(context, length).unwrap_or_default();
    let current = if on_the_left {
        format!("{name} {operator} {literal}")
    } else {
        format!("{literal} {operator} {name}")
    };
    if non_polymorphic_collection(context, length) {
        return;
    }
    let (Some(inner), Some(_)) = (
        length.field("receiver"),
        length.field("method"),
    ) else {
        return;
    };
    let dot = length
        .field("operator")
        .map_or(".", |operator| context.source.node_text(operator));
    // `replacement`: only the four zero-length shapes name the collection positively.
    let negated = !zero;
    let replacement = format!(
        "{}{}{dot}empty?",
        if negated { "!" } else { "" },
        context.source.node_text(inner)
    );
    let message = if zero {
        format!("Use `empty?` instead of `{current}`.")
    } else {
        format!("Use `!empty?` instead of `{current}`.")
    };
    offenses.push(
        context
            .offense(message, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
    );
}

/// `(call (...) {:length :size})`: a receiver, one of the two names, and no arguments.
fn length_call<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind_str() != "call" {
        return None;
    }
    let name = selector_of(context, node)?;
    if !matches!(name, "length" | "size") || !arguments(node).is_empty() {
        return None;
    }
    node.field("receiver")?;
    Some(node)
}

fn selector_of<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    node.field("method")
        .map(|method| context.source.node_text(method))
}

fn integer_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<i64> {
    if node.kind_str() != "integer" {
        return None;
    }
    context.source.node_text(node).parse().ok()
}

/// `non_polymorphic_collection?`: a size taken from a file handle is a byte count, not a collection.
fn non_polymorphic_collection(context: &RuleContext<'_>, length: Node<'_>) -> bool {
    let Some(receiver) = length.field("receiver") else {
        return false;
    };
    if receiver.kind_str() != "call" {
        return false;
    }
    let Some(inner) = receiver.field("receiver") else {
        return false;
    };
    let name = selector_of(context, receiver).unwrap_or_default();
    // `File.stat(f).size`, `File.new(f).size`, `Tempfile.open(f).size`, `File::Stat.new(f).size`.
    if matches!(name, "stat") && is_constant(context, inner, &["File"]) {
        return true;
    }
    if matches!(name, "new" | "open")
        && is_constant(context, inner, &["File", "Tempfile", "StringIO"])
    {
        return true;
    }
    if name == "new" && inner.kind_str() == "scope_resolution" {
        let scope = inner.field("scope");
        let named = inner
            .field("name")
            .map(|node| context.source.node_text(node));
        return named == Some("Stat")
            && scope.is_some_and(|scope| is_constant(context, scope, &["File"]));
    }
    false
}

fn is_constant(context: &RuleContext<'_>, node: Node<'_>, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| crate::rules::send_node::top_level_constant(node, name, context))
}
