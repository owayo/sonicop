//! `Style/MissingRespondToMissing`: `method_missing` without `respond_to_missing?` lies to
//! `respond_to?`.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "When using `method_missing`, define `respond_to_missing?`.";

/// The scopes a definition is looked up in. `sclass` is `class << self`.
const SCOPES: &[&str] = &["class", "module", "singleton_class"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name) = node.child_by_field_name("name") else {
            continue;
        };
        if context.source.node_text(name) != "method_missing" {
            continue;
        }
        // `respond_to_missing_elsewhere?` needs a project index, which a plain run never builds.
        if implements_respond_to_missing(context, node) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// `implements_respond_to_missing?`: a definition of the same kind, in the same scope.
fn implements_respond_to_missing(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let scope = enclosing_scope(node);
    let Some(root) = scope.or_else(|| node.parent()) else {
        return false;
    };
    let mut found = false;
    crate::rules::walk_named(root, &mut |candidate| {
        if found || candidate.kind() != node.kind() {
            return;
        }
        let named = candidate
            .child_by_field_name("name")
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
        if SCOPES.contains(&candidate.kind()) {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}
