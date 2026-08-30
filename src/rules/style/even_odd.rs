use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((left, selector, right)) = comparison(context, node) else {
            continue;
        };
        let equality = match context.source.node_text(selector) {
            "==" => true,
            "!=" => false,
            _ => continue,
        };
        let expected = match context.source.node_text(right) {
            "0" => 0,
            "1" => 1,
            _ => continue,
        };
        // `{(send $_ :% (int 2)) (begin (send $_ :% (int 2)))}`.
        let modulo = match left.kind_str() {
            "parenthesized_statements" => match super::nodes::children_in(left, context).as_slice() {
                [only] => *only,
                _ => continue,
            },
            _ => left,
        };
        let Some(base) = modulo_two(context, modulo) else {
            continue;
        };
        let method = match (expected, equality) {
            (0, true) | (1, false) => "even",
            _ => "odd",
        };
        offenses.push(
            context
                .offense(
                    format!("Replace with `Integer#{method}?`."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!("{}.{method}?", receiver_source(context, base)),
                    safe: true,
                }),
        );
    }
}

/// `(send $_ :% (int 2))`: the thing the remainder is taken of.
fn modulo_two<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    let (left, selector, right) = comparison(context, node)?;
    (context.source.node_text(selector) == "%" && context.source.node_text(right) == "2")
        .then_some(left)
}

/// The receiver, selector and single argument of an operator call written either way round.
fn comparison<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    let _ = context;
    match node.kind_str() {
        "binary" => Some((
            node.field("left")?,
            node.field("operator")?,
            node.field("right")?,
        )),
        _ => {
            if node.field("block").is_some() {
                return None;
            }
            let receiver = node.field("receiver")?;
            let selector = node.field("method")?;
            let arguments = node.field("arguments")?;
            match super::nodes::children(arguments).as_slice() {
                [only] => Some((receiver, selector, *only)),
                _ => None,
            }
        }
    }
}

/// `receiver_source`: an operator call binds looser than the predicate hung off it.
fn receiver_source(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let source = context.source.node_text(node);
    let operator = match node.kind_str() {
        "binary" => node
            .field("operator")
            .is_some_and(|operator| {
                super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        "unary" => node
            .field("operator")
            .is_some_and(|operator| {
                super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        "call" => node
            .field("method")
            .is_some_and(|method| method.kind_str() == "operator"),
        _ => false,
    };
    match operator {
        true => format!("({source})"),
        false => source.to_owned(),
    }
}
