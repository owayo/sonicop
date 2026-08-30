use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, symbol_name};
use crate::rules::send_node::named_children_of;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "ruby2_keywords" {
            continue;
        }
        let call_arguments = arguments(node);
        let Some(first) = call_arguments.first().map(|argument| argument.first()) else {
            continue;
        };
        match first.kind_str() {
            // `ruby2_keywords def m(...)`: the definition is right there, and the selector alone
            // is what gets reported.
            "method" => {
                if allowed_arguments(first.field("parameters")) {
                    continue;
                }
                let name = first
                    .field("name")
                    .map_or("", |name| context.source.node_text(name));
                offenses.push(context.offense(
                    format!("`ruby2_keywords` is unnecessary for method `{name}`."),
                    selector.byte_range(),
                ));
            }
            _ => {
                let Some(name) = symbol_name(first, context) else {
                    continue;
                };
                let Some(definition) = find_method_definition(node, name, context) else {
                    continue;
                };
                if allowed_arguments(definition) {
                    continue;
                }
                offenses.push(context.offense(
                    format!("`ruby2_keywords` is unnecessary for method `{name}`."),
                    node.byte_range(),
                ));
            }
        }
    }
}

/// `find_method_definition`: the nearest definition of that name written beside the call, giving
/// up at the class, module or singleton class the search started in.
fn find_method_definition<'tree>(
    node: Node<'tree>,
    name: &str,
    context: &'tree RuleContext<'_>,
) -> Option<Option<Node<'tree>>> {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        for child in named_children_of(ancestor, context) {
            if let Some(parameters) = definition_parameters(child, name, context) {
                return Some(parameters);
            }
        }
        if matches!(ancestor.kind_str(), "class" | "module" | "singleton_class") {
            return None;
        }
        current = ancestor.parent_of(context);
    }
    None
}

/// `method_definition`: `def name` or `define_method(:name) { ... }`, and the parameter list each
/// of them declares.
fn definition_parameters<'tree>(
    node: Node<'tree>,
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Option<Node<'tree>>> {
    if node.kind_str() == "method" {
        return node
            .field("name")
            .filter(|written| context.source.node_text(*written) == name)
            .map(|_| node.field("parameters"));
    }
    let block = node.field("block")?;
    if node
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "define_method")
    {
        return None;
    }
    let call_arguments = arguments(node);
    let first = call_arguments.first()?.first();
    (symbol_name(first, context) == Some(name)).then(|| block.field("parameters"))
}

/// `allowed_arguments?`: a rest parameter and no keyword parameter beside it, which is exactly the
/// signature `ruby2_keywords` exists for.
fn allowed_arguments(parameters: Option<Node<'_>>) -> bool {
    let Some(parameters) = parameters else {
        return false;
    };
    let children: Vec<Node<'_>> = named_children(parameters)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    if children.is_empty() {
        return false;
    }
    children
        .iter()
        .any(|child| child.kind_str() == "splat_parameter")
        && !children.iter().any(|child| {
            matches!(
                child.kind_str(),
                "keyword_parameter" | "hash_splat_parameter"
            )
        })
}
