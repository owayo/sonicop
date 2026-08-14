use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `RuboCop::AST::Node::COMPARISON_OPERATORS`.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["if", "unless", "elsif", "conditional"]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        if !is_comparison(context, condition) {
            continue;
        }
        let (Some(consequence), Some(alternative)) = (
            single_statement(node, "consequence"),
            single_statement(node, "alternative"),
        ) else {
            continue;
        };
        // `unless` puts its branches the other way round in the parser's tree.
        let (when_true, when_false) = match node.kind_str() {
            "unless" => (alternative, consequence),
            _ => (consequence, alternative),
        };
        let inverted = match (
            context.source.node_text(when_true),
            context.source.node_text(when_false),
        ) {
            ("true", "false") => false,
            ("false", "true") => true,
            _ => continue,
        };
        let mut expression = context.source.node_text(condition).to_owned();
        if inverted {
            expression = format!("!({expression})");
        }
        // `indented_else_node`: an `elsif` becomes the `else` of the conditional above it.
        let replacement = match node.kind_str() {
            "elsif" => {
                let width: usize = context
                    .setting::<i64>("IndentationWidth")
                    .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
                    .unwrap_or(2)
                    .max(0) as usize;
                format!(
                    "else\n{}{expression}",
                    " ".repeat(node.start_position().column + width)
                )
            }
            _ => expression,
        };
        let message = format!(
            "This conditional expression can just be replaced by `{}`.",
            match node.kind_str() {
                "elsif" => format!("\n{replacement}"),
                _ => replacement.clone(),
            }
        );
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

/// `(send _ {:== :=== :!= :<= :>= :< :>} _)`.
fn is_comparison(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let selector = match node.kind_str() {
        "binary" => node.field("operator"),
        "call" => node.field("method"),
        _ => None,
    };
    selector
        .is_some_and(|selector| COMPARISON_OPERATORS.contains(&context.source.node_text(selector)))
}

/// The one statement a branch holds, which is what `true` or `false` has to be on its own.
fn single_statement<'tree>(node: Node<'tree>, field: &str) -> Option<Node<'tree>> {
    let branch = node.field(field)?;
    match branch.kind_str() {
        "then" | "else" => match super::nodes::children(branch).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        _ => Some(branch),
    }
}
