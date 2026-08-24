use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `LENGTH_METHODS`: the three ways of asking how long something is.
const LENGTH_METHODS: [&str; 3] = ["length", "size", "count"];

/// `PRESERVING_METHODS`: the calls that hand back something of the same length, so the index the
/// caller computed still means the same element.
const PRESERVING_METHODS: [&str; 4] = ["sort", "reverse", "shuffle", "rotate"];

/// `arr[arr.length - 1]`, which `arr[-1]` says directly.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["element_reference", "call"]) {
        let Some((receiver, index)) = subscript(node, context) else {
            continue;
        };
        // `extract_range_from_begin`: a parenthesised index is unwrapped before it is read.
        let inner = unwrap_parentheses(index);
        if let Some(offense) = range_offense(context, receiver, index, inner) {
            offenses.push(offense);
            continue;
        }
        if let Some(offense) = simple_offense(context, receiver, index) {
            offenses.push(offense);
        }
    }
}

/// `handle_simple_index_pattern`: the index is `receiver.length - n` on its own.
fn simple_offense(
    context: &RuleContext<'_>,
    receiver: Node<'_>,
    index: Node<'_>,
) -> Option<Offense> {
    let (length_receiver, count) = length_subtraction(index, context)?;
    if !receivers_match(length_receiver, receiver, context) {
        return None;
    }
    let written = context.source.node_text(receiver);
    let current = format!("{written}[{}]", context.source.node_text(index));
    Some(
        context
            .offense(
                format!("Use `{written}[-{count}]` instead of `{current}`."),
                index.byte_range(),
            )
            .corrected_by(Edit {
                start: index.start_byte(),
                end: index.end_byte(),
                replacement: format!("-{count}"),
                safe: true,
            }),
    )
}

/// `range_with_length_subtraction?` and `handle_range_pattern`: the index is a range whose end is
/// the length minus something.
fn range_offense(
    context: &RuleContext<'_>,
    receiver: Node<'_>,
    index: Node<'_>,
    range: Node<'_>,
) -> Option<Offense> {
    if range.kind_str() != "range" {
        return None;
    }
    let (start, end) = (range.field("begin")?, range.field("end")?);
    if !preserving_method(start, context) {
        return None;
    }
    // `extract_inner_end`: the end may have been written in its own parentheses.
    let inner_end = unwrap_parentheses(end);
    let (length_receiver, count) = length_subtraction(inner_end, context)?;
    // `receivers_match_strict?`: for a range the two have to be written the same way.
    let length_receiver = length_receiver?;
    if !preserving_method(receiver, context)
        || context.source.node_text(length_receiver) != context.source.node_text(receiver)
    {
        return None;
    }
    let operator = if is_exclusive(range, context) {
        "..."
    } else {
        ".."
    };
    let written = context.source.node_text(receiver);
    let start_source = context.source.node_text(start);
    // `has_parentheses`: the index as a whole was parenthesised, and the message and the
    // replacement both keep them.
    let parenthesised = index.kind_str() == "parenthesized_statements";
    let end_source = if end.kind_str() == "parenthesized_statements" {
        context.source.node_text(end)
    } else {
        context.source.node_text(inner_end)
    };
    let without_parens = format!("{start_source}{operator}{end_source}");
    let (current, message_start, message_index) = if parenthesised {
        (
            format!("{written}[({without_parens})]"),
            format!("({start_source}"),
            format!("{count})"),
        )
    } else {
        (
            format!("{written}[{without_parens}]"),
            start_source.to_owned(),
            count.to_string(),
        )
    };
    let replacement = if parenthesised {
        format!("({start_source}{operator}-{count})")
    } else {
        format!("{start_source}{operator}-{count}")
    };
    Some(
        context
            .offense(
                format!(
                    "Use `{written}[{message_start}{operator}-{message_index}]` \
                     instead of `{current}`."
                ),
                end.byte_range(),
            )
            .corrected_by(Edit {
                start: index.start_byte(),
                end: index.end_byte(),
                replacement,
                safe: true,
            }),
    )
}

/// The receiver and the single index of a subscript, in both spellings of `[]`.
fn subscript<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    match node.kind_str() {
        "element_reference" => {
            // `RESTRICT_ON_SEND = %i[[]]`: `arr[i] = value` dispatches `[]=`, which this cop never
            // sees. The grammar writes the assignment around the same `element_reference`, so the
            // read and the write look alike until the parent is checked.
            if node.parent_of(context).is_some_and(|parent| {
                matches!(parent.kind_str(), "assignment" | "operator_assignment")
                    && parent
                        .field("left")
                        .is_some_and(|left| left.id() == node.id())
            }) {
                return None;
            }
            let object = node.field("object")?;
            let children = super::nodes::children(node);
            Some((object, *children.get(1)?))
        }
        _ => {
            let selector = node.field("method")?;
            if context.source.node_text(selector) != "[]" {
                return None;
            }
            let receiver = node.field("receiver")?;
            let arguments = super::nodes::children(node.field("arguments")?);
            Some((receiver, *arguments.first()?))
        }
    }
}

/// `(send (send $_ {:length :size :count}) :- (int $_))`, with the length's receiver and the number
/// taken off. A receiverless `length` is an identifier here and a `(send nil :length)` upstream.
fn length_subtraction<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Option<Node<'tree>>, i64)> {
    if node.kind_str() != "binary" {
        return None;
    }
    if context.source.node_text(node.field("operator")?) != "-" {
        return None;
    }
    let right = node.field("right")?;
    if right.kind_str() != "integer" {
        return None;
    }
    let count: i64 = context.source.node_text(right).parse().ok()?;
    // `negative_index&.positive?`.
    if count <= 0 {
        return None;
    }
    let left = node.field("left")?;
    match left.kind_str() {
        // `(send $_ {:length :size :count})` matches a call, and a local variable named `length`
        // is an `lvar` -- `length = do_something; self[length - 1]` subtracts from a number nobody
        // asked the receiver for, so there is no `[-1]` to rewrite it to.
        "identifier"
            if LENGTH_METHODS.contains(&context.source.node_text(left))
                && !context.variable_analysis().is_variable_reference(left) =>
        {
            Some((None, count))
        }
        "call" => {
            let selector = left.field("method")?;
            if !LENGTH_METHODS.contains(&context.source.node_text(selector)) {
                return None;
            }
            Some((left.field("receiver"), count))
        }
        _ => None,
    }
}

/// `receivers_match?`.
fn receivers_match(
    length_receiver: Option<Node<'_>>,
    array_receiver: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let Some(length_receiver) = length_receiver else {
        return array_receiver.kind_str() == "self";
    };
    if !preserving_method(array_receiver, context) || !preserving_method(length_receiver, context) {
        return false;
    }
    if context.source.node_text(length_receiver) == context.source.node_text(array_receiver) {
        return true;
    }
    base_receiver(array_receiver).is_some()
}

/// `extract_base_receiver`: the receiver at the bottom of a chain, or nothing when there is none.
fn base_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let receiver = call_receiver(node)?;
    match call_receiver(receiver) {
        Some(_) => base_receiver(receiver),
        None => Some(receiver),
    }
}

/// `preserving_method?`: a chain of length-preserving calls down to something that is not a call.
fn preserving_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(receiver) = call_receiver(node) else {
        return true;
    };
    node.field("method")
        .is_some_and(|selector| PRESERVING_METHODS.contains(&context.source.node_text(selector)))
        && preserving_method(receiver, context)
}

/// `node.receiver`, which upstream answers with `nil` for anything that is not a call.
fn call_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    (node.kind_str() == "call").then(|| node.field("receiver"))?
}

/// `range_node.erange_type?`: whether the range was written with three dots.
fn is_exclusive(range: Node<'_>, context: &RuleContext<'_>) -> bool {
    range
        .field("begin")
        .zip(range.field("end"))
        .is_some_and(|(begin, end)| {
            context.source.text()[begin.end_byte()..end.start_byte()].contains("...")
        })
}

/// `extract_range_from_begin` / `extract_inner_end`: what a pair of parentheses holds.
fn unwrap_parentheses<'tree>(node: Node<'tree>) -> Node<'tree> {
    if node.kind_str() != "parenthesized_statements" {
        return node;
    }
    match super::nodes::children(node).as_slice() {
        [only] => *only,
        _ => node,
    }
}
