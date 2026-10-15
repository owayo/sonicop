//! `Style/MissingRespondToMissing`: `method_missing` without `respond_to_missing?` lies to
//! `respond_to?`.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "When using `method_missing`, define `respond_to_missing?`.";

/// The scopes a definition is looked up in. `sclass` is `class << self`.
const SCOPES: &[&str] = &["class", "module", "singleton_class"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name) = node.field("name") else {
            continue;
        };
        if context.source.node_text(name) != "method_missing" {
            continue;
        }
        if implements_respond_to_missing(context, node)
            || respond_to_missing_elsewhere(context, node)
        {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

fn respond_to_missing_elsewhere(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(project) = context.project_index() else {
        return false;
    };
    let ancestors = crate::rules::lint::project_index_help::ancestors(node, context);
    let Some(namespace) = ancestors
        .iter()
        .find(|node| matches!(node.kind_str(), "class" | "module"))
    else {
        return false;
    };
    project.with(|index| {
        let Some(mut owner) = namespace.field("name").and_then(|name| {
            crate::rules::lint::project_index_help::resolve_constant(index, name, context)
        }) else {
            return false;
        };
        let singleton = node.kind_str() == "singleton_method"
            || ancestors
                .iter()
                .find(|node| SCOPES.contains(&node.kind_str()))
                .is_some_and(|scope| scope.kind_str() == "singleton_class");
        if singleton {
            let Some(scope) = index.singleton(owner) else {
                return false;
            };
            owner = scope;
        }
        index.member(owner, "respond_to_missing?()").is_some()
    })
}

/// `implements_respond_to_missing?`: a definition of the same kind, in the same scope.
fn implements_respond_to_missing(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let scope = enclosing_scope(node);
    let Some(root) = scope.or_else(|| node.parent_of(context)) else {
        return false;
    };
    let mut found = false;
    crate::rules::walk_named(root, context, &mut |candidate| {
        if found || candidate.kind_str() != node.kind_str() {
            return;
        }
        let named = candidate
            .field("name")
            .map(|name| context.source.node_text(name));
        if named == Some("respond_to_missing?") && same_scope(enclosing_scope(candidate), scope) {
            found = true;
        }
    });
    found
}

fn same_scope(left: Option<Node<'_>>, right: Option<Node<'_>>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.id() == right.id(),
        _ => false,
    }
}

fn enclosing_scope<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if SCOPES.contains(&candidate.kind_str()) {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}
