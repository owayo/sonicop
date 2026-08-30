use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Return `false` instead of `nil` in predicate methods.";

/// The node kinds `handle_if` walks into. `elsif` is an `if` node upstream, and a ternary is one
/// too.
const CONDITIONALS: [&str; 4] = ["if", "unless", "elsif", "conditional"];

/// A predicate method that answers `nil` instead of `false`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed_methods = context
        .setting::<Vec<String>>("AllowedMethods")
        .unwrap_or_default();
    let allowed_patterns: Vec<Regex> = context
        .setting::<Vec<String>>("AllowedPatterns")
        .unwrap_or_default()
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect();
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name) = node.field("name") else {
            continue;
        };
        // `node.predicate_method?`.
        let method = context.source.node_text(name);
        if !method.ends_with('?') {
            continue;
        }
        if allowed_methods.iter().any(|entry| entry == method)
            || allowed_patterns
                .iter()
                .any(|pattern| pattern.is_match(method))
        {
            continue;
        }
        let Some(body) = node.field("body").map(effective_body) else {
            continue;
        };
        // `body.each_descendant(:return)`: every `return` **below** the body. A body that is
        // itself a bare `return nil` is not a descendant of itself, so upstream misses it.
        let mut stack: Vec<Node<'_>> = super::nodes::children_in(body, context);
        while let Some(current) = stack.pop() {
            stack.extend(super::nodes::children_in(current, context));
            if current.kind_str() == "return" && returns_nil(current) {
                offenses.push(replacement(context, current, "return false"));
            }
        }
        implicit_return_values(context, Some(body), offenses);
    }
}

/// `handle_implicit_return_values`.
fn implicit_return_values(
    context: &RuleContext<'_>,
    node: Option<Node<'_>>,
    offenses: &mut Vec<Offense>,
) {
    if let Some(conditional) = last_node_of_kinds(node, &CONDITIONALS) {
        // `handle_if`: both branches answer for the method.
        implicit_return_values(context, conditional.field("consequence"), offenses);
        implicit_return_values(context, conditional.field("alternative"), offenses);
    }
    if let Some(literal) = last_node_of_kinds(node, &["nil"]) {
        offenses.push(replacement(context, literal, "false"));
    }
}

/// `last_node_of_type`: the node itself when it is of one of the kinds, or the last statement of a
/// statement list when that is.
fn last_node_of_kinds<'tree>(
    node: Option<Node<'tree>>,
    kinds: &[&str],
) -> Option<Node<'tree>> {
    let node = node?;
    if kinds.contains(&node.kind_str()) {
        return Some(node);
    }
    if !matches!(node.kind_str(), "body_statement" | "then" | "else") {
        return None;
    }
    let last = statements(node).pop()?;
    kinds.contains(&last.kind_str()).then_some(last)
}

/// `{(return) (return (nil))}`.
fn returns_nil(node: Node<'_>) -> bool {
    match super::nodes::children(node).as_slice() {
        [] => true,
        [list] if list.kind_str() == "argument_list" => {
            matches!(super::nodes::children(*list).as_slice(),
                     [only] if only.kind_str() == "nil")
        }
        _ => false,
    }
}

fn statements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    super::nodes::children(node)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

fn replacement(context: &RuleContext<'_>, node: Node<'_>, text: &str) -> Offense {
    context
        .offense(MSG, node.byte_range())
        .corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: text.to_owned(),
            safe: true,
        })
}

/// `node.body`: the single statement a body holds, or the statement list when it holds more --
/// which is the `begin` upstream builds for the same shape.
fn effective_body<'tree>(body: Node<'tree>) -> Node<'tree> {
    match statements(body).as_slice() {
        [only] => *only,
        _ => body,
    }
}
