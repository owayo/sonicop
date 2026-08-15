//! `Style/SelectByKind`: a `select` whose block is one `is_a?` check is a `grep`.

use tree_sitter::Node;

use super::select_by::{Selection, negation_operand, selection, test_arguments, test_method};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const METHODS: &[&str] = &["select", "filter", "find_all", "reject"];

/// `CLASS_CHECK_METHODS`.
const CLASS_CHECK_METHODS: &[&str] = &["is_a?", "kind_of?"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        let Some(selection) = selection(context, call, METHODS) else {
            continue;
        };
        let Some(test) = extract_send_node(&selection, context) else {
            continue;
        };
        let replacement = replacement(&selection, test, context);
        let message = format!(
            "Prefer `{replacement}` to `{}` with a kind check.",
            selection.method_name(context)
        );
        let rewrite = find_class_constant(test, context)
            .map(|constant| format!("{replacement}({})", context.source.node_text(constant)));
        offenses.push(selection.report(context, message, rewrite));
    }
}

/// `extract_send_node`: the check the block is, when it reads the element.
///
/// Unlike the regexp cop, `unwrap_negation` here does not look through parentheses, so
/// `!(x.is_a?(Foo))` matches nothing.
fn extract_send_node<'tree>(
    selection: &Selection<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let statement = selection.statement;
    let inner = negation_operand(statement, context).unwrap_or(statement);
    calls_argument(inner, selection, context).then_some(statement)
}

/// `(send (lvar %1) %CLASS_CHECK_METHODS _)`.
fn calls_argument(node: Node<'_>, selection: &Selection<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && test_method(node, context).is_some_and(|name| CLASS_CHECK_METHODS.contains(&name))
        && test_arguments(node).len() == 1
        && node.field("receiver").is_some_and(|receiver| {
            receiver.kind_str() == "identifier"
                && context.source.node_text(receiver) == selection.argument
        })
}

/// `replacement`.
fn replacement(
    selection: &Selection<'_>,
    statement: Node<'_>,
    context: &RuleContext<'_>,
) -> String {
    let negated = negation_operand(statement, context).is_some();
    match selection.keeps_matches(context) == negated {
        true => "grep_v".to_owned(),
        false => "grep".to_owned(),
    }
}

/// `find_class_constant`.
fn find_class_constant<'tree>(
    statement: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let inner = negation_operand(statement, context).unwrap_or(statement);
    test_arguments(inner).first().copied()
}
