use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_FOR_REPLACEMENT: &str = "Prefer `%<prefer>s` over `%<current>s`.";
const MSG_FOR_REDUNDANCY: &str = "Explicit non-nil checks are usually redundant.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let semantic_changes = context
        .setting::<bool>("IncludeSemanticChanges")
        .unwrap_or(false);
    // `nil_comparison_style`: with `Style/NilComparison` asking for `== nil`, rewriting to a
    // predicate would put the two cops at odds, so this one stands down.
    if !semantic_changes
        && context
            .setting_of::<bool>("Style/NilComparison", "Enabled")
            .unwrap_or(true)
        && context
            .setting_of::<String>("Style/NilComparison", "EnforcedStyle")
            .as_deref()
            == Some("comparison")
    {
        return;
    }

    for node in context.nodes_of_any(&["binary", "call", "unary"]) {
        let Some(receiver) = comparison_with_nil(context, node) else {
            continue;
        };
        if is_predicate_method_result(context, node) {
            continue;
        }
        let source = context.source.node_text(receiver);
        let replacement = match semantic_changes {
            true => source.to_owned(),
            false => format!("!{source}.nil?"),
        };
        let message = match semantic_changes {
            true => MSG_FOR_REDUNDANCY.to_owned(),
            false => MSG_FOR_REPLACEMENT
                .replace("%<prefer>s", &replacement)
                .replace("%<current>s", context.source.node_text(node)),
        };
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `not_equal_to_nil?`: `(send _ :!= nil)`. The two checks `IncludeSemanticChanges` adds are
/// `!x.nil?` and `unless x.nil?`, which the default configuration leaves alone.
fn comparison_with_nil<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<Node<'t>> {
    let (receiver, operator, argument) = match node.kind() {
        "binary" => (
            node.child_by_field_name("left")?,
            node.child_by_field_name("operator")?,
            node.child_by_field_name("right")?,
        ),
        "call" => {
            let arguments = super::nodes::children(node.child_by_field_name("arguments")?);
            let [only] = arguments.as_slice() else {
                return None;
            };
            (
                node.child_by_field_name("receiver")?,
                node.child_by_field_name("method")?,
                *only,
            )
        }
        _ => return None,
    };
    (context.source.node_text(operator) == "!=" && argument.kind() == "nil").then_some(receiver)
}

/// `on_def`'s `ignore_node`: the value a predicate method hands back is the one place an explicit
/// comparison earns its keep.
fn is_predicate_method_result(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "body_statement" {
        return false;
    }
    let Some(definition) = parent.parent() else {
        return false;
    };
    if !matches!(definition.kind(), "method" | "singleton_method") {
        return false;
    }
    if definition
        .child_by_field_name("name")
        .is_none_or(|name| !context.source.node_text(name).ends_with('?'))
    {
        return false;
    }
    // The body is the statement itself when it stands alone, and its last statement otherwise.
    super::nodes::children(parent)
        .last()
        .is_some_and(|last| last.id() == node.id())
}
