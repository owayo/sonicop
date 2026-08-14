use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Prefer modules to classes with only class methods.";

/// `"class".len()` and `"end".len()`, the two keywords whose spans are fixed.
const CLASS_LENGTH: usize = 5;
const END_LENGTH: usize = 3;

/// What a `casgn` and friends look like here: an assignment to a name rather than to something a
/// method call stands behind.
const ASSIGNMENT_TARGETS: [&str; 7] = [
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
    "constant",
    "scope_resolution",
    "left_assignment_list",
];

/// A class that only ever holds class methods, which a module says better.
///
/// Upstream has a second guard, `subclassed_in_project?`, that consults a project-wide constant
/// index. The index is opt-in through `AllCops: UseProjectIndex` and is not built here, so that
/// guard is never taken -- which is also what upstream does with the default configuration.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("class") {
        if node.field("superclass").is_some() {
            continue;
        }
        let elements = class_elements(node);
        if elements.is_empty() || !elements.iter().all(|child| convertible(*child, context)) {
            continue;
        }
        let Some(name) = node.field("name") else {
            continue;
        };
        let mut edits = vec![
            // `corrector.replace(class_node.loc.keyword, 'module')`.
            Edit {
                start: node.start_byte(),
                end: node.start_byte() + CLASS_LENGTH,
                replacement: "module".to_owned(),
                safe: true,
            },
            // `corrector.insert_after(class_node.loc.name, "\nmodule_function\n")`.
            Edit {
                start: name.end_byte(),
                end: name.end_byte(),
                replacement: "\nmodule_function\n".to_owned(),
                safe: true,
            },
        ];
        for element in &elements {
            match element.kind_str() {
                // `def self.foo` loses its receiver and the dot.
                "singleton_method" => {
                    if let (Some(object), Some(method)) =
                        (element.field("object"), element.field("name"))
                    {
                        edits.push(Edit {
                            start: object.start_byte(),
                            end: method.start_byte(),
                            replacement: String::new(),
                            safe: true,
                        });
                    }
                }
                // `class << self` loses its head and its `end`.
                "singleton_class" => {
                    if let Some(value) = element.field("value") {
                        edits.push(Edit {
                            start: element.start_byte(),
                            end: value.end_byte(),
                            replacement: String::new(),
                            safe: true,
                        });
                    }
                    edits.push(Edit {
                        start: element.end_byte() - END_LENGTH,
                        end: element.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    });
                }
                _ => {}
            }
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// One element of a class body counts when it is a public class method, an `extend`, an assignment
/// to a name, or a `class << self` that holds only those.
fn convertible(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "singleton_method" => is_public(node, context),
        "singleton_class" => sclass_convertible(node, context),
        "assignment" | "operator_assignment" => node
            .field("left")
            .is_some_and(|target| ASSIGNMENT_TARGETS.contains(&target.kind_str())),
        // `extend_call?` asks only about the selector, so `Foo.extend Bar` counts too.
        "call" => node
            .field("method")
            .is_some_and(|selector| context.source.node_text(selector) == "extend"),
        _ => false,
    }
}

/// `sclass_convertible_to_module?`.
fn sclass_convertible(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    class_elements(node).iter().all(|child| {
        is_public(*child, context)
            && (child.kind_str() == "method"
                || (matches!(child.kind_str(), "assignment" | "operator_assignment")
                    && child
                        .field("left")
                        .is_some_and(|target| ASSIGNMENT_TARGETS.contains(&target.kind_str()))))
    })
}

/// `node_visibility(node) == :public`.
///
/// Only the block form matters here: an inline `private def x` makes the *call* the element, and a
/// call is not one of the shapes a convertible class may hold.
fn is_public(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    let siblings = super::nodes::children(parent);
    let Some(position) = siblings.iter().position(|child| child.id() == node.id()) else {
        return true;
    };
    !siblings[..position].iter().rev().any(|sibling| {
        sibling.kind_str() == "call"
            && sibling.field("receiver").is_none()
            && sibling.field("arguments").is_none()
            && sibling.field("method").is_some_and(|selector| {
                matches!(context.source.node_text(selector), "private" | "protected")
            })
    })
}

/// `class_elements`: the statements a class or `class << self` body holds.
fn class_elements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    node.field("body").map_or_else(Vec::new, |body| {
        super::nodes::children(body)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect()
    })
}
