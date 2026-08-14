use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const STD_STREAMS: &[&str] = &["STDIN", "STDOUT", "STDERR"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["constant", "scope_resolution"]) {
        // The target of `STDOUT = io` is a `casgn` upstream and holds no `const` node at all, so
        // `on_const` never sees it.
        if is_assignment_target(node) {
            continue;
        }
        let Some(name) = stream_name(context, node) else {
            continue;
        };
        let global = format!("${}", name.to_lowercase());
        // `const_to_gvar_assignment?`: `$stdout = STDOUT` is the assignment that makes the global
        // mean the constant, so it has to keep naming it.
        if node.kind_str() == "constant" && assigns_matching_global(context, node, &global) {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Use `{global}` instead of `{name}`."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: global,
                    safe: true,
                }),
        );
    }
}

/// The stream this constant names, once the qualified spellings are ruled out.
///
/// `namespaced?` keeps `Foo::STDOUT` but not `::STDOUT`, which still means the one stream.
fn stream_name<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    let name = match node.kind_str() {
        "scope_resolution" => {
            if node.field("scope").is_some() {
                return None;
            }
            node.field("name")?
        }
        _ => {
            // The name half of a qualified constant is reached through the resolution above.
            if node.parent_of(context).is_some_and(|parent| {
                parent.kind_str() == "scope_resolution"
                    && parent
                        .field("name")
                        .is_some_and(|inner| inner.id() == node.id())
            }) {
                return None;
            }
            node
        }
    };
    let text = context.source.node_text(name);
    STD_STREAMS.contains(&text).then_some(text)
}

/// Whether this constant is being assigned rather than read. A constant assignment is a `casgn`
/// upstream, whose name is a symbol rather than a node, so nothing about it reaches `on_const`.
fn is_assignment_target(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| match parent.kind_str() {
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_some_and(|left| left.id() == node.id()),
        "left_assignment_list" | "rest_assignment" => true,
        _ => false,
    })
}

fn assigns_matching_global(context: &RuleContext<'_>, node: Node<'_>, global: &str) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        parent.kind_str() == "assignment"
            && parent
                .field("right")
                .is_some_and(|right| right.id() == node.id())
            && parent.field("left").is_some_and(|left| {
                left.kind_str() == "global_variable" && context.source.node_text(left) == global
            })
    })
}
