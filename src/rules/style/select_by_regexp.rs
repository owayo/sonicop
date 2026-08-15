//! `Style/SelectByRegexp`: a `select` whose block is one regexp match is a `grep`.

use tree_sitter::Node;

use super::select_by::{
    Selection, calls_argument, negation_operand, selection, test_arguments, test_method,
    test_receiver, unwrap_negation,
};
use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const METHODS: &[&str] = &["select", "filter", "find_all", "reject"];

/// `REGEXP_METHODS`.
const REGEXP_METHODS: &[&str] = &["match?", "=~"];

/// The version that gave `Enumerable` its `filter` alias.
const FILTER_VERSION: RubyVersion = RubyVersion::new(2, 6);

/// The version that gave `Enumerable` its `grep_v`.
const GREP_V_VERSION: RubyVersion = RubyVersion::new(2, 3);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        let Some(selection) = selection(context, call, METHODS) else {
            continue;
        };
        if context.target_ruby_version() < FILTER_VERSION
            && selection.method_name(context) == "filter"
        {
            continue;
        }
        let Some(test) = extract_send_node(&selection, context) else {
            continue;
        };
        if match_predicate_without_receiver(test, context) {
            continue;
        }
        let replacement = replacement(&selection, test, context);
        if context.target_ruby_version() < GREP_V_VERSION && replacement == "grep_v" {
            continue;
        }
        let message = format!(
            "Prefer `{replacement}` to `{}` with a regexp match.",
            selection.method_name(context)
        );
        let rewrite = find_regexp(&selection, test, context)
            .map(|regexp| format!("{replacement}({})", context.source.node_text(regexp)));
        offenses.push(selection.report(context, message, rewrite));
    }
}

/// `extract_send_node`: the test the block is, when it is one that reads the element.
fn extract_send_node<'tree>(
    selection: &Selection<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let statement = selection.statement;
    let inner = unwrap_negation(statement, context);
    if !is_regexp_test(inner, context) {
        return None;
    }
    calls_argument(inner, &selection.argument, context).then_some(statement)
}

/// `(send _ %REGEXP_METHODS _)`, `(send _ %REGEXP_METHODS_NEGATED _)` and `match-with-lvasgn`.
fn is_regexp_test(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "binary" => matches!(test_method(node, context), Some("=~") | Some("!~")),
        "call" => {
            test_method(node, context).is_some_and(|name| REGEXP_METHODS.contains(&name))
                && test_arguments(node).len() == 1
        }
        _ => false,
    }
}

/// `negated?`.
fn negated(statement: Node<'_>, context: &RuleContext<'_>) -> bool {
    negation_operand(statement, context).is_some()
        || test_method(unwrap_negation(statement, context), context) == Some("!~")
}

/// `replacement`.
fn replacement(
    selection: &Selection<'_>,
    statement: Node<'_>,
    context: &RuleContext<'_>,
) -> String {
    let negated = negated(statement, context);
    match selection.keeps_matches(context) == negated {
        true => "grep_v".to_owned(),
        false => "grep".to_owned(),
    }
}

/// `match_predicate_without_receiver?`: `match?(/re/)` reads the element as an argument, so what
/// it matched against is not the element.
fn match_predicate_without_receiver(statement: Node<'_>, context: &RuleContext<'_>) -> bool {
    let inner = unwrap_negation(statement, context);
    inner.kind_str() == "call"
        && test_method(inner, context) == Some("match?")
        && inner.field("receiver").is_none()
}

/// `find_regexp`: whichever side of the test is not the element.
fn find_regexp<'tree>(
    selection: &Selection<'tree>,
    statement: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let inner = unwrap_negation(statement, context);
    // `match-with-lvasgn`: the parser builds one only when a regexp literal is written on the
    // left, and there the regexp is the node's first child.
    if is_match_with_lvasgn(inner, context) {
        return inner.field("left");
    }
    let receiver = test_receiver(inner);
    let arguments = test_arguments(inner);
    if receiver.is_some_and(|receiver| is_element(receiver, selection, context)) {
        return arguments.first().copied();
    }
    arguments
        .first()
        .is_some_and(|first| is_lvar(*first, selection, context))
        .then_some(receiver)?
}

/// `match-with-lvasgn`: `/re/ =~ x`, which binds the regexp's named captures as locals.
fn is_match_with_lvasgn(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && test_method(node, context) == Some("=~")
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "regex")
}

/// `inner.receiver.lvar_type? && (block is implicit || receiver.source == block param)`.
fn is_element(node: Node<'_>, selection: &Selection<'_>, context: &RuleContext<'_>) -> bool {
    is_lvar(node, selection, context)
        && (!selection.declared || context.source.node_text(node) == selection.argument)
}

/// `node.lvar_type?`. The implicit parameters are locals upstream and bare names here.
fn is_lvar(node: Node<'_>, selection: &Selection<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "identifier"
        && (context.source.node_text(node) == selection.argument
            || LocalVariables::new(context).is_lvar(node))
}
