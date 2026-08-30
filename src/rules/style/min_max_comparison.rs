//! `Style/MinMaxComparison`: a conditional that picks the larger of two values is `max`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const GREATER_OPERATORS: &[&str] = &[">", ">="];
const LESS_OPERATORS: &[&str] = &["<", "<="];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["if", "unless", "elsif", "conditional"]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let Some((left, operator, right)) = comparison(condition, context) else {
            continue;
        };
        let (Some(consequence), Some(alternative)) =
            (branch(node, "consequence"), branch(node, "alternative"))
        else {
            continue;
        };
        // `unless` reaches its branches the other way round.
        let (if_branch, else_branch) = match node.kind_str() {
            "unless" => (alternative, consequence),
            _ => (consequence, alternative),
        };
        let same = |left: Node<'_>, right: Node<'_>| super::nodes::same_tree(context, left, right);
        let preferred = if same(left, if_branch) && same(right, else_branch) {
            if GREATER_OPERATORS.contains(&operator) {
                "max"
            } else {
                "min"
            }
        } else if same(left, else_branch) && same(right, if_branch) {
            if LESS_OPERATORS.contains(&operator) {
                "max"
            } else {
                "min"
            }
        } else {
            continue;
        };
        let replacement = format!(
            "[{}, {}].{preferred}",
            context.source.node_text(left),
            context.source.node_text(right),
        );
        let offense = context.offense(format!("Use `{replacement}` instead."), node.byte_range());
        // An `elsif` cannot be replaced whole: what is left of the chain has to keep its `else`,
        // so the branch's own head is dropped and only its body is rewritten.
        let edits = match (node.kind_str(), node.field("alternative")) {
            ("elsif", Some(otherwise)) => vec![
                Edit {
                    start: node.start_byte(),
                    end: otherwise.start_byte(),
                    replacement: String::new(),
                    safe: true,
                },
                Edit {
                    start: else_branch.start_byte(),
                    end: else_branch.end_byte(),
                    replacement,
                    safe: true,
                },
            ],
            _ => vec![Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }],
        };
        offenses.push(offense.corrected_by_all(edits));
    }
}

/// `{(send $_lhs $COMPARISON_OPERATORS $_rhs) (begin (send $_lhs $COMPARISON_OPERATORS $_rhs))}`.
fn comparison<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<(Node<'tree>, &'static str, Node<'tree>)> {
    // `(begin ...)`: a condition written in parentheses.
    let node = match node.kind_str() {
        "parenthesized_statements" => match super::nodes::children_in(node, context).as_slice() {
            [only] => *only,
            _ => return None,
        },
        _ => node,
    };
    if node.kind_str() != "binary" {
        return None;
    }
    let operator = context.source.node_text(node.field("operator")?);
    let operator = GREATER_OPERATORS
        .iter()
        .chain(LESS_OPERATORS)
        .find(|known| **known == operator)?;
    Some((node.field("left")?, operator, node.field("right")?))
}

/// The one statement a branch holds. A branch of more than one is a `begin` upstream, which never
/// equals one of the compared values.
fn branch<'tree>(node: Node<'tree>, field: &str) -> Option<Node<'tree>> {
    let branch = node.field(field)?;
    match branch.kind_str() {
        "then" | "else" => match super::nodes::children(branch).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        // An `elsif` is the nested conditional upstream hands out as the else branch.
        "elsif" => None,
        _ => Some(branch),
    }
}
