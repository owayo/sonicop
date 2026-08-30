use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::blocks::{BLOCK_KINDS, BlockArgs};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::lint::node_equality::identical;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;
use crate::rules::support;

/// `SELECT_METHODS`.
const SELECT_METHODS: [&str; 3] = ["select", "filter", "find_all"];

/// `CANDIDATE_METHODS`, which is also `RESTRICT_ON_SEND`.
const CANDIDATE_METHODS: [&str; 4] = ["select", "filter", "find_all", "reject"];

/// The node kinds holding a run of statements that upstream's parser reads as a `begin`.
///
/// A `begin ... end` written out is a `kwbegin` there rather than a `begin`, and only the
/// statements it guards with a `rescue` or an `ensure` sit in a `begin` of their own.
const SEQUENCES: &[&str] = &[
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    "ensure",
    "do",
    "parenthesized_statements",
    "begin",
];

/// Two statements that keep the elements a predicate accepts and the ones it turns down, which one
/// `partition` answers with in a single pass.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for parent in context.nodes_of_any(SEQUENCES) {
        if !holds_a_begin(parent) {
            continue;
        }
        let statements: Vec<Node<'_>> = super::nodes::children_in(parent, context);
        for window in statements.windows(2) {
            let [sibling_container, container] = window else {
                continue;
            };
            let Some(node) = candidate(*container)
                .filter(|node| CANDIDATE_METHODS.contains(&selector_of(*node, context)))
            else {
                continue;
            };
            let Some(sibling) = candidate(*sibling_container) else {
                continue;
            };
            if !same_receiver(node, sibling, context) {
                continue;
            }
            let Some(pair) = matching_pair(node, sibling, context, &locals) else {
                continue;
            };
            let message = format!(
                "Use `partition` instead of consecutive `{}` and `{}` calls.",
                selector_of(sibling, context),
                selector_of(node, context)
            );
            let offense = context.offense(message, container.byte_range());
            // `both_lvasgn?`: only a pair that names both halves can be rewritten.
            let (Some(kept), Some(dropped)) = (
                assigned_name(*sibling_container, context),
                assigned_name(*container, context),
            ) else {
                offenses.push(offense);
                continue;
            };
            let (select_var, reject_var, partition) = match pair {
                // `complementary_variable_order` / `select_node_for`.
                Pair::Complementary => {
                    if SELECT_METHODS.contains(&selector_of(sibling, context)) {
                        (kept, dropped, sibling)
                    } else {
                        (dropped, kept, node)
                    }
                }
                // `negation_partition_args`.
                Pair::Negated { node_is_negated } => {
                    let is_select = SELECT_METHODS.contains(&selector_of(node, context));
                    let partition = if node_is_negated { sibling } else { node };
                    if is_select != node_is_negated {
                        (dropped, kept, partition)
                    } else {
                        (kept, dropped, partition)
                    }
                }
            };
            let replacement = format!(
                "{select_var}, {reject_var} = {}",
                partition_call(partition, context)
            );
            let removed = support::whole_lines(container.byte_range(), context);
            // Upstream removes the whole of the second statement's lines and rewrites the first
            // one, which is two corrections over the same text when the pair shares a line. Its
            // rewriter refuses that, and the exception it raises leaves the offense unreported.
            if removed.start < sibling_container.end_byte() {
                continue;
            }
            offenses.push(offense.corrected_by_all([
                Edit {
                    start: sibling_container.start_byte(),
                    end: sibling_container.end_byte(),
                    replacement,
                    safe: true,
                },
                Edit {
                    start: removed.start,
                    end: removed.end,
                    replacement: String::new(),
                    safe: true,
                },
            ]));
        }
    }
}

/// Which of the two shapes upstream lets a pair through by.
enum Pair {
    /// `complementary_pair?` with `equivalent_predicate?`: a `select` beside a `reject` saying the
    /// same thing.
    Complementary,
    /// `negated_predicate?`: the same selector twice, one predicate the negation of the other.
    Negated { node_is_negated: bool },
}

/// Whether the run of statements is one upstream's parser wraps in a `begin`.
///
/// Everything but a `begin ... end` written out is; that one is a `kwbegin`, and only what a
/// `rescue` or an `ensure` guards inside it becomes a `begin`.
fn holds_a_begin(parent: Node<'_>) -> bool {
    if parent.kind_str() != "begin" {
        return true;
    }
    super::nodes::children(parent)
        .into_iter()
        .any(|child| matches!(child.kind_str(), "rescue" | "ensure" | "else"))
}

/// `extract_candidate`: the filtering call of a statement, whether or not it is assigned.
fn candidate<'tree>(container: Node<'tree>) -> Option<Node<'tree>> {
    let node = if is_assignment(container) {
        container.field("right")?
    } else {
        container
    };
    if node.kind_str() != "call" {
        return None;
    }
    let has_block = block_of(node).is_some();
    // `node.last_argument&.block_pass_type?`.
    let block_pass = last_argument(node).is_some_and(|last| last.kind_str() == "block_argument");
    (has_block || block_pass).then_some(node)
}

/// `node.receiver == sibling.receiver`, which two receiverless calls also answer yes to.
fn same_receiver(node: Node<'_>, sibling: Node<'_>, context: &RuleContext<'_>) -> bool {
    match (node.field("receiver"), sibling.field("receiver")) {
        (Some(left), Some(right)) => identical(left, right, context),
        (None, None) => true,
        _ => false,
    }
}

/// `matching_pair?`.
fn matching_pair(
    node: Node<'_>,
    sibling: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Pair> {
    let (first, second) = (selector_of(node, context), selector_of(sibling, context));
    // `complementary_pair?`.
    let complementary = (SELECT_METHODS.contains(&first) && second == "reject")
        || (first == "reject" && SELECT_METHODS.contains(&second));
    if complementary && equivalent_predicate(node, sibling, context, locals) {
        return Some(Pair::Complementary);
    }
    if first != second {
        return None;
    }
    // `negated_predicate?`.
    let (Some(block), Some(other)) = (block_of(node), block_of(sibling)) else {
        return None;
    };
    if !same_block_kind(block, other, context, locals) {
        return None;
    }
    if negated_body(single_statement(block), single_statement(other), context) {
        return Some(Pair::Negated {
            node_is_negated: true,
        });
    }
    negated_body(single_statement(other), single_statement(block), context).then_some(
        Pair::Negated {
            node_is_negated: false,
        },
    )
}

/// `equivalent_predicate?`.
fn equivalent_predicate(
    node: Node<'_>,
    sibling: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    match (block_of(node), block_of(sibling)) {
        (Some(block), Some(other)) => same_block_contents(block, other, context, locals),
        (Some(_), None) => block_matches_block_pass(node, sibling, context),
        (None, Some(_)) => block_matches_block_pass(sibling, node, context),
        // `node1.last_argument == node2.last_argument`.
        (None, None) => match (last_argument(node), last_argument(sibling)) {
            (Some(left), Some(right)) => identical(left, right, context),
            _ => false,
        },
    }
}

/// `same_block_contents?`.
fn same_block_contents(
    block: Node<'_>,
    other: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    same_block_kind(block, other, context, locals) && same_body(block, other, context)
}

/// `block1.type == block2.type` with, for a block written with bars, `block1.arguments ==
/// block2.arguments`.
///
/// The three node types upstream builds for a block are one node kind here, so what separates them
/// is what the block declares: bars of its own, a numbered parameter, or an `it`.
fn same_block_kind(
    block: Node<'_>,
    other: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    match (
        BlockArgs::of(block, context, locals),
        BlockArgs::of(other, context, locals),
    ) {
        (BlockArgs::Written(left), BlockArgs::Written(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(&right)
                    .all(|(left, right)| identical(*left, *right, context))
        }
        (BlockArgs::Numbered(_), BlockArgs::Numbered(_)) | (BlockArgs::It, BlockArgs::It) => true,
        _ => false,
    }
}

/// `block1.body == block2.body`, where a body of several statements is one `begin` upstream.
fn same_body(block: Node<'_>, other: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (left, right) = (statements(block), statements(other));
    left.len() == right.len()
        && left
            .iter()
            .zip(&right)
            .all(|(left, right)| identical(*left, *right, context))
}

/// `block_matches_block_pass?`: `{ |x| x.foo }` says the same thing as `&:foo`.
fn block_matches_block_pass(block: Node<'_>, send: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(method) = symbol_proc_method(block, context) else {
        return false;
    };
    let Some(argument) = last_argument(send) else {
        return false;
    };
    let Some(symbol) = super::nodes::children_in(argument, context).into_iter().next() else {
        return false;
    };
    send_node::symbol_name(symbol, context) == Some(method)
}

/// `symbol_proc_method?`: `(block _ (args (arg _name)) (send (lvar _name) $_method_name))`.
fn symbol_proc_method<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let block = block_of(node)?;
    let parameters = super::nodes::children_in(block.field("parameters")?, context);
    let [only] = parameters.as_slice() else {
        return None;
    };
    if only.kind_str() != "identifier" {
        return None;
    }
    let body = single_statement(block)?;
    if body.kind_str() != "call"
        || body.field("arguments").is_some()
        || body.field("block").is_some()
    {
        return None;
    }
    let receiver = body.field("receiver")?;
    if receiver.kind_str() != "identifier"
        || context.source.node_text(receiver) != context.source.node_text(*only)
    {
        return None;
    }
    Some(context.source.node_text(body.field("method")?))
}

/// `negated_body?`: one body is the other with a `!` in front. `not` is spelled `:!` there too.
fn negated_body(
    body: Option<Node<'_>>,
    other: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> bool {
    let (Some(body), Some(other)) = (body, other) else {
        return false;
    };
    // `(send X :!)`: the grammar spells it `unary` for `!x` and `call` for `x.!`.
    send_node::negation(body, context)
        .is_some_and(|found| identical(found.operand, other, context))
}

/// `build_partition_call`: the same call with its selector swapped for `partition`.
fn partition_call(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let Some(selector) = node.field("method") else {
        return context.source.node_text(node).to_owned();
    };
    let text = context.source.text();
    format!(
        "{}partition{}",
        &text[node.start_byte()..selector.start_byte()],
        &text[selector.end_byte()..node.end_byte()]
    )
}

fn selector_of<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    node.field("method")
        .map_or("", |selector| context.source.node_text(selector))
}

fn last_argument<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("arguments")
        .map(super::nodes::children)
        .and_then(|arguments| arguments.last().copied())
}

/// The block written on the call, if it has one.
fn block_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("block")
        .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
}

/// The statements a block body holds, which is nothing at all when it has no body.
fn statements<'tree>(block: Node<'tree>) -> Vec<Node<'tree>> {
    block
        .field("body")
        .map_or_else(Vec::new, super::nodes::children)
}

/// The one statement a block body holds, which is what upstream's `body` is a single node for.
fn single_statement<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    match statements(block).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

fn is_assignment(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "assignment" | "operator_assignment")
}

/// `container.lvasgn_type?` together with the name it assigns.
fn assigned_name<'a>(container: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if container.kind_str() != "assignment" {
        return None;
    }
    let target = container.field("left")?;
    (target.kind_str() == "identifier").then(|| context.source.node_text(target))
}

