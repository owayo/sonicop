use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_IMPLICIT: &str = "Omit the error class when rescuing `StandardError` by itself.";
const MSG_EXPLICIT: &str = "Avoid rescuing without specifying an error class.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let explicit = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "explicit");

    // A `rescue` modifier is a node of its own in the grammar, so it never reaches this loop.
    for node in context.nodes_of("rescue") {
        let Some(keyword) = node.child(0) else {
            continue;
        };
        let exceptions = node.child_by_field_name("exceptions");
        match (explicit, exceptions) {
            // `rescue_without_error_class?`.
            (true, None) => offenses.push(
                context
                    .offense(MSG_EXPLICIT, keyword.byte_range())
                    .corrected_by(Edit {
                        start: keyword.end_byte(),
                        end: keyword.end_byte(),
                        replacement: " StandardError".to_owned(),
                        safe: true,
                    }),
            ),
            // `rescue_standard_error?`: the list names `StandardError` and nothing else.
            (false, Some(list)) if only_standard_error(context, list) => offenses.push(
                context
                    .offense(MSG_IMPLICIT, keyword.start_byte()..list.end_byte())
                    .corrected_by(Edit {
                        start: keyword.end_byte(),
                        end: list.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    }),
            ),
            _ => {}
        }
    }
}

/// `(array (const {nil? cbase} :StandardError))`: the one name, unqualified or written from the
/// root.
fn only_standard_error(context: &RuleContext<'_>, list: Node<'_>) -> bool {
    let exceptions = super::nodes::children(list);
    let [only] = exceptions.as_slice() else {
        return false;
    };
    match only.kind() {
        "constant" => context.source.node_text(*only) == "StandardError",
        "scope_resolution" => {
            only.child_by_field_name("scope").is_none()
                && only
                    .child_by_field_name("name")
                    .is_some_and(|name| context.source.node_text(name) == "StandardError")
        }
        _ => false,
    }
}
