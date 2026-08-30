//! `Style/SelectByRange`: a `select` whose block is one range check is a `grep`.

use tree_sitter::Node;

use super::select_by::{
    SELECT_METHODS, Selection, negation_operand, selection, test_arguments, test_method,
    unwrap_negation,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const METHODS: &[&str] = &["select", "filter", "find_all", "reject", "find", "detect"];

/// `FIND_METHODS`, which take the first match rather than all of them.
const FIND_METHODS: &[&str] = &["find", "detect"];

/// The two ways a range is asked whether it holds something.
const COVER_METHODS: &[&str] = &["cover?", "include?"];

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
            "Prefer `{replacement}` to `{}` with a range check.",
            selection.method_name(context)
        );
        let grep = match replacement.contains("grep_v") {
            true => "grep_v",
            false => "grep",
        };
        let suffix = match replacement.contains(".first") {
            true => ".first",
            false => "",
        };
        let literal = find_range(test, context);
        offenses.push(selection.report(
            context,
            message,
            literal.map(|literal| format!("{grep}({literal}){suffix}")),
        ));
    }
}

/// `extract_send_node`: the check the block is, when it reads the element.
fn extract_send_node<'tree>(
    selection: &Selection<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let statement = selection.statement;
    let inner = unwrap_negation(statement, context);
    calls_argument_in_range_check(inner, selection, context).then_some(statement)
}

/// `calls_lvar_in_range_check?`: `x.between?(a, b)` or `(a..b).cover?(x)`.
fn calls_argument_in_range_check(
    node: Node<'_>,
    selection: &Selection<'_>,
    context: &RuleContext<'_>,
) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let Some(method) = test_method(node, context) else {
        return false;
    };
    let arguments = test_arguments(node);
    match method {
        // `(send (lvar %1) :between? _ _)`.
        "between?" => {
            arguments.len() == 2
                && node
                    .field("receiver")
                    .is_some_and(|receiver| names_argument(receiver, selection, context))
        }
        // `(send {range (begin range)} {:cover? :include?} (lvar %1))`.
        name if COVER_METHODS.contains(&name) => {
            arguments.len() == 1
                && names_argument(arguments[0], selection, context)
                && node.field("receiver").is_some_and(is_range)
        }
        _ => false,
    }
}

fn names_argument(node: Node<'_>, selection: &Selection<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "identifier" && context.source.node_text(node) == selection.argument
}

/// `{range (begin range)}`.
fn is_range(node: Node<'_>) -> bool {
    match node.kind_str() {
        "range" => true,
        "parenthesized_statements" => super::nodes::children(node)
            .first()
            .is_some_and(|inner| inner.kind_str() == "range"),
        _ => false,
    }
}

/// `replacement`, which for `find` and `detect` names the whole `grep(...).first` rewrite.
fn replacement(
    selection: &Selection<'_>,
    statement: Node<'_>,
    context: &RuleContext<'_>,
) -> String {
    let negated = negation_operand(statement, context).is_some();
    let method = selection.method_name(context);
    if FIND_METHODS.contains(&method.as_str()) {
        return match negated {
            true => "grep_v(...).first".to_owned(),
            false => "grep(...).first".to_owned(),
        };
    }
    match SELECT_METHODS.contains(&method.as_str()) == negated {
        true => "grep_v".to_owned(),
        false => "grep".to_owned(),
    }
}

/// `find_range`: the range the check was written against, as source text.
fn find_range(statement: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let inner = unwrap_negation(statement, context);
    let arguments = test_arguments(inner);
    if test_method(inner, context) == Some("between?") {
        let (Some(min), Some(max)) = (arguments.first(), arguments.get(1)) else {
            return None;
        };
        return Some(format!(
            "{}..{}",
            context.source.node_text(*min),
            context.source.node_text(*max)
        ));
    }
    let receiver = inner.field("receiver")?;
    let receiver = match receiver.kind_str() {
        "parenthesized_statements" => super::nodes::children_in(receiver, context).first().copied()?,
        _ => receiver,
    };
    Some(context.source.node_text(receiver).to_owned())
}
