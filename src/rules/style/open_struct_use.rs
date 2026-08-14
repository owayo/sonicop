use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str =
    "Avoid using `OpenStruct`; use `Struct`, `Hash`, a class or test doubles instead.";

/// `(const {nil? (cbase)} :OpenStruct)`.
///
/// Upstream reads one `const` node per constant, and the scope hangs inside it. Here `::OpenStruct`
/// is a `scope_resolution` and the bare `OpenStruct` is a `constant`, so the two spellings are
/// walked separately. `Foo::OpenStruct` has a scope and never matches, while the `OpenStruct` of
/// `OpenStruct::Foo` is a constant of its own and does.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["constant", "scope_resolution"]) {
        let reported = match node.kind_str() {
            "constant" => {
                if context.source.node_text(node) != "OpenStruct" {
                    continue;
                }
                // As the name of a `scope_resolution` the constant is reached through that scope,
                // which upstream spells as a `const` whose own scope is not `nil` or `cbase`.
                if is_field_of(node, "name", "scope_resolution") {
                    continue;
                }
                node
            }
            _ => {
                if node.field("scope").is_some() {
                    continue;
                }
                let Some(name) = node.field("name") else {
                    continue;
                };
                if context.source.node_text(name) != "OpenStruct" {
                    continue;
                }
                node
            }
        };
        // `custom_class_or_module_definition?`: the constant being defined is not a use of it.
        if is_field_of(reported, "name", "class") || is_field_of(reported, "name", "module") {
            continue;
        }
        // A constant *assignment* is a `casgn` upstream, which holds the name as a symbol rather
        // than as a `const` node, so the cop never sees it.
        if is_assignment_target(reported) {
            continue;
        }
        offenses.push(context.offense(MSG, reported.byte_range()));
    }
}

/// Whether `node` fills the `field` of a parent of kind `parent_kind`.
fn is_field_of(node: Node<'_>, field: &str, parent_kind: &str) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind_str() == parent_kind
            && parent
                .field(field)
                .is_some_and(|value| value.id() == node.id())
    })
}

/// Whether `node` is what is being assigned to, including one element of a multiple assignment.
fn is_assignment_target(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind_str() {
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_some_and(|left| left.id() == node.id()),
        "left_assignment_list" | "rest_assignment" => true,
        _ => false,
    }
}
