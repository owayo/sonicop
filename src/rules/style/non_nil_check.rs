use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

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
        if semantic_changes
            && !is_predicate_method_result(context, node)
            && let Some(edits) = semantic_change(context, node)
        {
            offenses.push(
                context
                    .offense(MSG_FOR_REDUNDANCY, node.byte_range())
                    .corrected_by_all(edits),
            );
            continue;
        }
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

/// The two checks `IncludeSemanticChanges` adds: `not_and_nil_check?` (`!x.nil?`) and
/// `unless_and_nil_check?` (`unless x.nil?`). Both drop the `nil?` call itself, which is the
/// semantic change the option is named for -- `x` is falsy for `false` too.
fn semantic_change(context: &RuleContext<'_>, node: Node<'_>) -> Option<Vec<Edit>> {
    // `autocorrect_non_nil`: `!x.nil?` becomes `x`, and a receiverless `!nil?` becomes `self`.
    // `not x.nil?` is the same `:!` send as `!x.nil?`. The grammar gives `!` an `operator` field
    // and leaves `not` as an anonymous child, so both spellings have to be read.
    let negation = node.kind_str() == "unary"
        && node
            .field("operator")
            .or_else(|| node.child(0))
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"));
    if negation {
        let operand = node.field("operand")?;
        if !is_nil_predicate(context, operand) {
            return None;
        }
        let replacement = match operand.field("receiver") {
            Some(receiver) => context.source.node_text(receiver).to_owned(),
            None => "self".to_owned(),
        };
        return Some(vec![Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement,
            safe: true,
        }]);
    }

    // `autocorrect_unless_nil`: the keyword becomes `if` and the condition loses its `nil?`.
    if !is_nil_predicate(context, node) {
        return None;
    }
    let parent = node.parent_of(context)?;
    if !matches!(parent.kind_str(), "unless" | "unless_modifier") {
        return None;
    }
    if parent.field("condition")?.id() != node.id() {
        return None;
    }
    let keyword = (0..parent.child_count())
        .filter_map(|index| parent.child(index as u32))
        .find(|child| context.source.node_text(*child) == "unless")?;
    let receiver = node.field("receiver")?;
    Some(vec![
        Edit {
            start: keyword.start_byte(),
            end: keyword.end_byte(),
            replacement: "if".to_owned(),
            safe: true,
        },
        Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: context.source.node_text(receiver).to_owned(),
            safe: true,
        },
    ])
}

/// `nil_check?`: `(send _ :nil?)` with no arguments.
fn is_nil_predicate(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "call"
        && node
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "nil?")
        && node.field("arguments").is_none()
}

/// `not_equal_to_nil?`: `(send _ :!= nil)`. The two checks `IncludeSemanticChanges` adds are
/// `!x.nil?` and `unless x.nil?`, which the default configuration leaves alone.
fn comparison_with_nil<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<Node<'t>> {
    let (receiver, operator, argument) = match node.kind_str() {
        "binary" => (
            node.field("left")?,
            node.field("operator")?,
            node.field("right")?,
        ),
        "call" => {
            let arguments = super::nodes::children_in(node.field("arguments")?, context);
            let [only] = arguments.as_slice() else {
                return None;
            };
            (
                node.field("receiver")?,
                node.field("method")?,
                *only,
            )
        }
        _ => return None,
    };
    (context.source.node_text(operator) == "!=" && argument.kind_str() == "nil").then_some(receiver)
}

/// `on_def`'s `ignore_node`: the value a predicate method hands back is the one place an explicit
/// comparison earns its keep.
fn is_predicate_method_result(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    if parent.kind_str() != "body_statement" {
        return false;
    }
    let Some(definition) = parent.parent_of(context) else {
        return false;
    };
    if !matches!(definition.kind_str(), "method" | "singleton_method") {
        return false;
    }
    if definition
        .field("name")
        .is_none_or(|name| !context.source.node_text(name).ends_with('?'))
    {
        return false;
    }
    // The body is the statement itself when it stands alone, and its last statement otherwise.
    super::nodes::children_in(parent, context)
        .last()
        .is_some_and(|last| last.id() == node.id())
}
