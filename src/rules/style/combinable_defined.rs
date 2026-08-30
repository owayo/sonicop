//! `Style/CombinableDefined`: `defined?(Foo::Bar)` already answers for `Foo`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::final_pos;

const MSG: &str = "Combine nested `defined?` calls.";

/// `OPERATORS`.
const OPERATORS: &[&str] = &["&&", "and"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("binary") {
        if !is_and(node, context) {
            continue;
        }
        let terms = terms(node, context);
        if terms.is_empty()
            || !terms
                .iter()
                .all(|term| defined_subject(*term, context).is_some())
        {
            continue;
        }
        // `defined_calls` keeps the subjects that name something reachable through a namespace.
        let calls: Vec<Node<'_>> = terms
            .iter()
            .filter_map(|term| defined_subject(*term, context))
            .filter(|subject| match subject.kind_str() {
                "constant" | "scope_resolution" | "call" => true,
                // A receiverless call of no arguments is a bare identifier here and a `send`
                // upstream; a name the scope assigned is a variable read, and no call at all.
                "identifier" => !locals.is_lvar(*subject),
                _ => false,
            })
            .collect();
        let namespaces: Vec<Node<'_>> = calls.iter().filter_map(|call| namespace(*call)).collect();
        // `add_offense` is keyed on the range, so only the first term that is a namespace of
        // another is ever reported -- and corrected -- for this `and`.
        let Some(call) = calls.iter().find(|call| {
            namespaces
                .iter()
                .any(|space| super::nodes::same_tree(context, **call, *space))
        }) else {
            continue;
        };
        let Some(range) = removal(context, *call) else {
            continue;
        };
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// `remove_term`: the term and the operator that joins it to the rest.
fn removal(context: &RuleContext<'_>, subject: Node<'_>) -> Option<std::ops::Range<usize>> {
    // `term = term.parent until term.parent.and_type?`.
    let mut term = subject;
    while !context
        .parent(term)
        .is_some_and(|parent| is_and(parent, context))
    {
        term = context.parent(term)?;
    }
    let parent = context.parent(term)?;
    let text = context.source.text();
    let range = if parent
        .field("right")
        .is_some_and(|right| right.id() == term.id())
    {
        // `rhs_range_to_remove`: back up to the operator in front of the term.
        let mut position = term.start_byte();
        while !OPERATORS
            .iter()
            .any(|operator| text[position..].starts_with(operator))
        {
            position = position.checked_sub(1)?;
        }
        position.checked_sub(1)?..term.end_byte()
    } else {
        // `lhs_range_to_remove`: run forward to the end of the operator behind the term.
        let mut position = term.end_byte();
        while !OPERATORS
            .iter()
            .any(|operator| text[..position.min(text.len())].ends_with(operator))
        {
            position += 1;
            if position > text.len() {
                return None;
            }
        }
        term.start_byte()..position
    };
    // `range_with_surrounding_space(side: :right, newlines: false)`.
    Some(range.start..final_pos(text, range.end, true, false, false, false))
}

/// `node.parent.and_type?`: `&&` and `and`, which are one node type upstream.
fn is_and(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "&&" | "and"))
}

/// `terms`: every descendant an `and` joins, which is what all of them have to be `defined?` for.
fn terms<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if is_and(current, context) {
            for child in [current.field("left"), current.field("right")]
                .into_iter()
                .flatten()
            {
                if !is_and(child, context) {
                    found.push(child);
                }
            }
        }
        let mut children = super::nodes::children_in(current, context);
        children.reverse();
        stack.extend(children);
    }
    found.sort_by_key(|term| term.start_byte());
    found
}

/// `defined_node.first_argument`: what a `defined?` was asked about.
///
/// The keyword is a `unary` here and a node type of its own upstream, where the parentheses around
/// its argument are part of the keyword rather than a `begin` around the argument.
fn defined_subject<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "unary" {
        return None;
    }
    let operator = node.child(0)?;
    if context.source.node_text(operator) != "defined?" {
        return None;
    }
    let operand = node.field("operand")?;
    if operand.kind_str() != "parenthesized_statements" {
        return Some(operand);
    }
    match super::nodes::children(operand).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `namespaces`: what a constant is reached through, or what a call is written on.
fn namespace<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "scope_resolution" => node.field("scope"),
        "call" => node.field("receiver"),
        _ => None,
    }
}
