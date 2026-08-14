//! `Style/ComparableClamp`: pinning a value between two bounds is `clamp`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `minimum_target_ruby_version 2.4`: `Comparable#clamp` arrived in 2.4.
const MINIMUM: RubyVersion = RubyVersion::new(2, 4);

const MSG_MIN_MAX: &str = "Use `Comparable#clamp` instead.";

/// Which end of the range a branch's bound is.
#[derive(Clone, Copy, PartialEq)]
enum Bound {
    Min,
    Max,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of_any(&["if", "elsif"]) {
        check_conditional(context, node, offenses);
    }
    for node in context.nodes_of("call") {
        check_min_max(context, node, offenses);
    }
}

/// `if_elsif_else_condition?`: the eight ways of writing "below the minimum, above the maximum,
/// otherwise the value itself".
fn check_conditional(context: &RuleContext<'_>, node: Node<'_>, offenses: &mut Vec<Offense>) {
    let (Some(condition), Some(first), Some(otherwise)) = (
        node.field("condition"),
        branch(node.field("consequence")),
        node.field("alternative"),
    ) else {
        return;
    };
    if otherwise.kind_str() != "elsif" {
        return;
    }
    let (Some(inner_condition), Some(second), Some(last)) = (
        otherwise.field("condition"),
        branch(otherwise.field("consequence")),
        otherwise.field("alternative"),
    ) else {
        return;
    };
    if last.kind_str() != "else" {
        return;
    }
    let Some(value) = branch(Some(last)) else {
        return;
    };
    let (Some(outer), Some(inner)) = (
        bound(condition, first, value, context),
        bound(inner_condition, second, value, context),
    ) else {
        return;
    };
    if outer == inner {
        return;
    }
    let (min, max) = match outer {
        Bound::Min => (first, second),
        Bound::Max => (second, first),
    };
    let prefer = format!(
        "{}.clamp({}, {})",
        parenthesize_if_needed(value, context),
        context.source.node_text(min),
        context.source.node_text(max),
    );
    let message = format!("Use `{prefer}` instead of `if/elsif/else`.");
    // An `elsif` cannot be replaced whole: the chain above it still needs an `else`.
    let edits = if node.kind_str() == "elsif" {
        vec![
            Edit {
                start: node.start_byte(),
                end: node.start_byte(),
                replacement: "else\n".to_owned(),
                safe: true,
            },
            Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!(
                    "{}{prefer}",
                    " ".repeat(node.start_position().column + indentation_width(context))
                ),
                safe: true,
            },
        ]
    } else {
        vec![Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: prefer,
            safe: true,
        }]
    };
    offenses.push(
        context
            .offense(message, node.byte_range())
            .corrected_by_all(edits),
    );
}

/// `array_min_max?`: `[[a, b].max, c].min` and its three mirror images.
fn check_min_max(context: &RuleContext<'_>, node: Node<'_>, offenses: &mut Vec<Offense>) {
    let outer = match context.source.node_text(match node.field("method") {
        Some(name) => name,
        None => return,
    }) {
        "min" => "max",
        "max" => "min",
        _ => return,
    };
    if !arguments(node).is_empty() || node.field("block").is_some() {
        return;
    }
    let Some(array) = node.field("receiver").filter(|r| r.kind_str() == "array") else {
        return;
    };
    let elements = super::nodes::children(array);
    if elements.len() != 2 {
        return;
    }
    let inner = |element: Node<'_>| {
        element.kind_str() == "call"
            && element
                .field("method")
                .is_some_and(|name| context.source.node_text(name) == outer)
            && arguments(element).is_empty()
            && element.field("block").is_none()
            && element.field("receiver").is_some_and(|receiver| {
                receiver.kind_str() == "array" && super::nodes::children(receiver).len() == 2
            })
    };
    if inner(elements[0]) || inner(elements[1]) {
        offenses.push(context.offense(MSG_MIN_MAX, node.byte_range()));
    }
}

/// Which bound a branch names, given the condition that leads to it and the value being clamped.
fn bound(
    condition: Node<'_>,
    body: Node<'_>,
    value: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<Bound> {
    if condition.kind_str() != "binary" {
        return None;
    }
    let (left, right) = (condition.field("left")?, condition.field("right")?);
    let same = |left: Node<'_>, right: Node<'_>| super::nodes::same_tree(context, left, right);
    match context.source.node_text(condition.field("operator")?) {
        // `x < min` and `max < x`.
        "<" if same(left, value) && same(right, body) => Some(Bound::Min),
        "<" if same(right, value) && same(left, body) => Some(Bound::Max),
        // `min > x` and `x > max`.
        ">" if same(right, value) && same(left, body) => Some(Bound::Min),
        ">" if same(left, value) && same(right, body) => Some(Bound::Max),
        _ => None,
    }
}

/// `Alignment#indentation`: the node's own column plus one level.
fn indentation_width(context: &RuleContext<'_>) -> usize {
    context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2)
        .max(0) as usize
}

/// The one statement a branch holds.
fn branch<'tree>(clause: Option<Node<'tree>>) -> Option<Node<'tree>> {
    match super::nodes::children(clause?).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `parenthesize_if_needed`: what has to be wrapped before a method can be called on it.
fn parenthesize_if_needed(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let source = context.source.node_text(node);
    if matches!(
        node.kind_str(),
        "binary"
            | "unary"
            | "if"
            | "unless"
            | "elsif"
            | "conditional"
            | "range"
            | "assignment"
            | "operator_assignment"
    ) {
        return format!("({source})");
    }
    source.to_owned()
}
