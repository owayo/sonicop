use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const FORBID_MIXED: &str = "Do not use mixed logical operators in an `unless`.";
const FORBID_ALL: &str = "Do not use any logical operator in an `unless`.";

/// The two spellings of each logical operator, symbol first.
const AND_OPERATORS: [&str; 2] = ["&&", "and"];
const OR_OPERATORS: [&str; 2] = ["||", "or"];

/// `unless` conditions built out of `&&` / `||` / `and` / `or`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let forbid_all = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "forbid_logical_operators");
    for node in context.nodes_of_any(&["unless", "unless_modifier"]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        if forbid_all {
            // `logical_operator?`: the condition itself is an `and` or an `or`.
            if logical_operator(condition, context).is_some() {
                offenses.push(context.offense(FORBID_ALL, node.byte_range()));
            }
        } else if mixed(condition, context) {
            offenses.push(context.offense(FORBID_MIXED, node.byte_range()));
        }
    }
}

/// `mixed_logical_operator?`: the four checks upstream runs, in order.
fn mixed(condition: Node<'_>, context: &RuleContext<'_>) -> bool {
    let top = logical_operator(condition, context);
    // `or_with_and?` / `and_with_or?`: an `or` holding an `and` below it, or the reverse.
    if let Some(operator) = top {
        let other = if is_or(operator) {
            AND_OPERATORS
        } else {
            OR_OPERATORS
        };
        if descendant_operators(condition, &other, context)
            .next()
            .is_some()
        {
            return true;
        }
    }
    // `mixed_precedence_and?` / `mixed_precedence_or?`: one kind spelled both ways. The condition's
    // own operator joins the list only when it is of that kind, so a logical operator buried in an
    // argument still counts on its own.
    for spellings in [AND_OPERATORS, OR_OPERATORS] {
        let mut written: Vec<&str> = descendant_operators(condition, &spellings, context).collect();
        if let Some(operator) = top.filter(|operator| spellings.contains(operator)) {
            written.push(operator);
        }
        if !(written.iter().all(|entry| *entry == spellings[0])
            || written.iter().all(|entry| *entry == spellings[1]))
        {
            return true;
        }
    }
    false
}

/// The operator of the node when it is a logical one.
fn logical_operator<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if node.kind_str() != "binary" {
        return None;
    }
    let operator = context.source.node_text(node.field("operator")?);
    (AND_OPERATORS.contains(&operator) || OR_OPERATORS.contains(&operator)).then_some(operator)
}

fn is_or(operator: &str) -> bool {
    OR_OPERATORS.contains(&operator)
}

/// The operators of every descendant that is one of `wanted`.
fn descendant_operators<'a, 'tree>(
    node: Node<'tree>,
    wanted: &'a [&'a str],
    context: &'a RuleContext<'_>,
) -> impl Iterator<Item = &'a str> + 'a
where
    'tree: 'a,
{
    let mut stack: Vec<Node<'tree>> = super::nodes::children(node);
    std::iter::from_fn(move || {
        while let Some(current) = stack.pop() {
            stack.extend(super::nodes::children(current));
            if let Some(operator) = logical_operator(current, context)
                && wanted.contains(&operator)
            {
                return Some(operator);
            }
        }
        None
    })
}
