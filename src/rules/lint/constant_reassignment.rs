use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, string_text, symbol_name};
use crate::rules::support::spurious_assignment_list;
use crate::rules::send_node::named_children_of;

/// The parts of a constant path, as `each_path` walks them.
struct ConstantPath {
    /// `constant_namespaces`: the `const` links of the path, which is what the message names.
    namespaces: Vec<String>,
    /// `node.absolute?`: the path opens with a `::`.
    absolute: bool,
    /// The namespace written directly in front of the name of an absolute path, as
    /// `node.namespace.source` spells it.
    absolute_namespace: Option<String>,
    name: String,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut definitions: HashSet<String> = HashSet::new();
    // The commissioner visits nodes in depth-first pre-order, which is what decides whether a
    // definition was already seen when an assignment is reached.
    let mut stack = vec![(context.root_node(), Vec::<String>::new())];
    let mut order = Vec::new();
    while let Some((node, namespaces)) = stack.pop() {
        order.push((node, namespaces.clone()));
        let inner = match node.kind_str() {
            "class" | "module" => match node
                .field("name")
                .and_then(|name| constant_path(name, context))
            {
                Some(path) => {
                    let mut inner = namespaces.clone();
                    // `ancestor_namespaces` asks each enclosing declaration only for its
                    // `identifier.short_name`. A compact or absolute namespace therefore
                    // contributes its final component without replacing the lexical ancestors.
                    inner.push(path.name.clone());
                    inner
                }
                None => namespaces.clone(),
            },
            _ => namespaces.clone(),
        };
        let mut children = named_children_of(node, context);
        children.reverse();
        for child in children {
            stack.push((child, inner.clone()));
        }
    }
    for (node, namespaces) in order {
        match node.kind_str() {
            "class" | "module" => {
                if !unconditional_definition(node, context) {
                    continue;
                }
                if let Some(name) = definition_name(node, &namespaces, context) {
                    definitions.insert(name);
                }
            }
            "assignment" => {
                if !simple_assignment(node, context) {
                    continue;
                }
                for (target, range) in assignment_targets(node) {
                    let Some(path) = constant_path(target, context) else {
                        continue;
                    };
                    let qualified = qualified_name(&path, &namespaces);
                    let display = display_name(&path);
                    if !definitions.insert(qualified) {
                        offenses.push(context.offense(
                            format!("Constant `{display}` is already assigned in this namespace."),
                            range.clone(),
                        ));
                    }
                }
            }
            // `remove_const` takes the constant back out of the namespace it was defined in.
            "call" => {
                if namespaces.is_empty() {
                    continue;
                }
                let Some(name) = remove_const_argument(node, context) else {
                    continue;
                };
                definitions.remove(&join(&namespaces, &name));
            }
            _ => {}
        }
    }
}

/// Every `casgn` the assignment writes, with the span upstream reports it at.
///
/// A lone target carries its value, so the `casgn` is the whole assignment; a target inside a
/// multiple assignment has none, and the node is just the name.
fn assignment_targets<'tree>(node: Node<'tree>) -> Vec<(Node<'tree>, std::ops::Range<usize>)> {
    let Some(left) = node.field("left") else {
        return Vec::new();
    };
    if is_constant_target(left) {
        return vec![(left, node.byte_range())];
    }
    if left.kind_str() != "left_assignment_list" || spurious_assignment_list(left) {
        return Vec::new();
    }
    named_children(left)
        .into_iter()
        .filter(|child| is_constant_target(*child))
        .map(|child| (child, child.byte_range()))
        .collect()
}

fn is_constant_target(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "constant" | "scope_resolution")
}

/// `each_path`, refused for anything a scope could compute at run time.
fn constant_path(node: Node<'_>, context: &RuleContext<'_>) -> Option<ConstantPath> {
    match node.kind_str() {
        "constant" => Some(ConstantPath {
            namespaces: Vec::new(),
            absolute: false,
            absolute_namespace: None,
            name: context.source.node_text(node).to_owned(),
        }),
        "scope_resolution" => {
            let name = context.source.node_text(node.field("name")?).to_owned();
            let Some(scope) = node.field("scope") else {
                // `::NAME`, whose namespace is the `cbase` itself.
                return Some(ConstantPath {
                    namespaces: Vec::new(),
                    absolute: true,
                    absolute_namespace: None,
                    name,
                });
            };
            match scope.kind_str() {
                "self" => Some(ConstantPath {
                    namespaces: Vec::new(),
                    absolute: false,
                    absolute_namespace: None,
                    name,
                }),
                "constant" | "scope_resolution" => {
                    let inner = constant_path(scope, context)?;
                    let mut namespaces = inner.namespaces;
                    namespaces.push(inner.name);
                    Some(ConstantPath {
                        namespaces,
                        absolute: inner.absolute,
                        absolute_namespace: inner
                            .absolute
                            .then(|| context.source.node_text(scope).to_owned()),
                        name,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// `fully_qualified_constant_name`.
fn qualified_name(path: &ConstantPath, namespaces: &[String]) -> String {
    if path.absolute {
        return match &path.absolute_namespace {
            Some(namespace) => format!("{namespace}::{}", path.name),
            None => format!("::{}", path.name),
        };
    }
    let mut parts = namespaces.to_vec();
    parts.extend(path.namespaces.iter().cloned());
    join(&parts, &path.name)
}

/// `constant_display_name`.
fn display_name(path: &ConstantPath) -> String {
    let mut parts = path.namespaces.clone();
    parts.push(path.name.clone());
    parts.join("::")
}

fn join(namespaces: &[String], name: &str) -> String {
    let mut parts = vec![String::new()];
    parts.extend(namespaces.iter().cloned());
    parts.push(name.to_owned());
    parts.join("::")
}

/// `definition_name`: what a `class` or `module` puts into the namespace.
fn definition_name(
    node: Node<'_>,
    namespaces: &[String],
    context: &RuleContext<'_>,
) -> Option<String> {
    // `fully_qualified_constant_name` is the same function for a definition as for an assignment:
    // `class ::A::FooError` registers `::A::FooError`, which is what `A::FooError = …` looks up.
    // Dropping the namespace of an absolute path made the two spell the constant differently.
    let path = constant_path(node.field("name")?, context)?;
    Some(qualified_name(&path, namespaces))
}

/// `unconditional_definition?`: nothing but a class, a module or a statement list stands above it.
fn unconditional_definition(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if !is_statement_list(ancestor) && !matches!(ancestor.kind_str(), "class" | "module") {
            return false;
        }
        current = ancestor.parent_of(context);
    }
    true
}

/// `simple_assignment?`: nothing that could make the write conditional stands above it. Reaching a
/// `class` or a `module` answers yes without looking any further up.
fn simple_assignment(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "class" | "module") {
            return true;
        }
        let allowed = is_statement_list(ancestor)
            || is_literal(ancestor)
            || matches!(
                ancestor.kind_str(),
                "assignment" | "left_assignment_list" | "right_assignment_list"
            )
            || is_freeze_call(ancestor, context);
        if !allowed {
            return false;
        }
        current = ancestor.parent_of(context);
    }
    true
}

/// The nodes the parser reads as a `begin`, plus the program itself.
fn is_statement_list(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "program" | "begin" | "body_statement" | "block_body" | "then" | "else"
    )
}

fn is_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "array" | "hash" | "pair" | "string_array" | "symbol_array" | "string" | "range"
    )
}

fn is_freeze_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        // `on_send` は `csend` に呼ばれない。`&.freeze` は本家からは freeze に見えないので、
        // その中の代入は「凍った定数の中」として扱われない。
        && crate::rules::send_node::is_plain_send(node, context)
        && node
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "freeze")
}

/// `(send {nil? self} :remove_const ({sym str} $_))`.
fn remove_const_argument(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let method = node.field("method")?;
    if context.source.node_text(method) != "remove_const" {
        return None;
    }
    if node
        .field("receiver")
        .is_some_and(|receiver| receiver.kind_str() != "self")
    {
        return None;
    }
    let call_arguments = arguments(node);
    let [only] = call_arguments.as_slice() else {
        return None;
    };
    let argument = only.first();
    symbol_name(argument, context)
        .map(str::to_owned)
        .or_else(|| {
            (argument.kind_str() == "string").then(|| string_text(argument, context).to_owned())
        })
}
