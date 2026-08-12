use tree_sitter::Node;

use super::access_modifier::{bare_send_name, begin_statements, statements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::symbol_name;

const ALTERNATIVE_PRIVATE: &str =
    "`private_class_method` or `private` inside a `class << self` block";
const ALTERNATIVE_PROTECTED: &str = "`protected` inside a `class << self` block";

/// `access_modifier?`: a bare modifier other than `module_function`, which says nothing about
/// singleton methods either way.
const ACCESS_MODIFIERS: [&str; 3] = ["public", "protected", "private"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_class` and `on_module` only. A `class << self` body is where these modifiers do work, so
    // upstream leaves it alone.
    for node in context.nodes_of_any(&["class", "module"]) {
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        // `check_node` returns unless the body is a `begin`: a body of one statement cannot hold
        // both the modifier and the definition it fails to govern.
        let Some(children) = begin_statements(body) else {
            continue;
        };
        let mut ignored = None;
        ineffective_modifier(context, body, &children, &mut ignored, None, offenses);
    }
}

/// The walk of one statement list, carrying the modifier last seen.
///
/// `ignored_methods` is computed once, from the class body, the first time a definition or a
/// `kwbegin` needs it -- upstream's `||=` leaves it alone in the recursion because the caller has
/// always filled it in by then. The modifier, by contrast, is taken by value: what a `kwbegin`
/// declares does not leak back out to the statements after it.
fn ineffective_modifier<'tree>(
    context: &RuleContext<'tree>,
    node: Node<'tree>,
    children: &[Node<'tree>],
    ignored: &mut Option<Vec<String>>,
    mut modifier: Option<Node<'tree>>,
    offenses: &mut Vec<Offense>,
) {
    for &child in children {
        match child.kind() {
            "singleton_method" => {
                let ignored =
                    ignored.get_or_insert_with(|| private_class_method_names(context, node));
                if correct_visibility(context, child, modifier, ignored) {
                    continue;
                }
                let (Some(modifier), Some(keyword)) = (modifier, child.child(0)) else {
                    continue;
                };
                offenses.push(context.offense(message(context, modifier), keyword.byte_range()));
            }
            "begin" => {
                ignored.get_or_insert_with(|| private_class_method_names(context, node));
                let inner = statements(child).unwrap_or_default();
                ineffective_modifier(context, child, &inner, ignored, modifier, offenses);
            }
            _ if bare_send_name(child, context)
                .is_some_and(|name| ACCESS_MODIFIERS.contains(&name)) =>
            {
                modifier = Some(child);
            }
            _ => {}
        }
    }
}

/// `correct_visibility?`: no modifier and a `public` one both leave singleton methods alone, and a
/// name already passed to `private_class_method` is private however it was defined.
fn correct_visibility(
    context: &RuleContext<'_>,
    definition: Node<'_>,
    modifier: Option<Node<'_>>,
    ignored: &[String],
) -> bool {
    let Some(modifier) = modifier else {
        return true;
    };
    if bare_send_name(modifier, context) == Some("public") {
        return true;
    }
    definition.child_by_field_name("name").is_some_and(|name| {
        let name = context.source.node_text(name);
        ignored.iter().any(|method| method == name)
    })
}

/// The names `private_class_method` was called with anywhere under `node`.
///
/// Upstream keeps only the arguments that are `basic_literal?` and compares their `value` against
/// the definition's method name, which is a symbol -- so a name written as a string never matches
/// however it was spelled.
fn private_class_method_names(context: &RuleContext<'_>, node: Node<'_>) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "call"
            && current.child_by_field_name("receiver").is_none()
            && current
                .child_by_field_name("method")
                .is_some_and(|method| context.source.node_text(method) == "private_class_method")
            && let Some(arguments) = current.child_by_field_name("arguments")
        {
            let mut cursor = arguments.walk();
            for argument in arguments.named_children(&mut cursor) {
                if let Some(name) = symbol_name(argument, context) {
                    names.push(name.to_owned());
                }
            }
        }
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
    }
    names
}

fn message(context: &RuleContext<'_>, modifier: Node<'_>) -> String {
    let visibility = bare_send_name(modifier, context).unwrap_or_default();
    let alternative = if visibility == "private" {
        ALTERNATIVE_PRIVATE
    } else {
        ALTERNATIVE_PROTECTED
    };
    let (line, _) = context.source.line_column(modifier.start_byte());
    format!(
        "`{visibility}` (on line {line}) does not make singleton methods {visibility}. \
         Use {alternative} instead."
    )
}
