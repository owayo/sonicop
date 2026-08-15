use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::is_string;

/// The keyword the grammar folds into an ordinary prefix operator.
const DEFINED: &str = "defined?";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("unary") {
        let Some(keyword) = node.child(0) else {
            continue;
        };
        if context.source.node_text(keyword) != DEFINED {
            continue;
        }
        let Some(argument) = first_argument(node, keyword) else {
            continue;
        };
        let Some(kind) = literal_kind(argument, context) else {
            continue;
        };
        let message =
            format!("Calling `defined?` with a {kind} argument will always return a truthy value.");
        offenses.push(context.offense(message, node.byte_range()));
    }
}

/// `node.first_argument`: what `defined?` was handed.
///
/// The parens of `defined?(x)` belong to the keyword and never reach the cop, but the ones of
/// `defined? (x)` are an expression of their own -- upstream reads the second as a `begin`, which
/// is not a literal and so is never reported. Only the space between tells them apart, so the
/// parentheses are unwrapped only where they sit against the keyword.
fn first_argument<'tree>(node: Node<'tree>, keyword: Node<'tree>) -> Option<Node<'tree>> {
    let operand = node.field("operand")?;
    if operand.kind_str() == "parenthesized_statements"
        && operand.start_byte() == keyword.end_byte()
    {
        return operand.named_child(0);
    }
    Some(operand)
}

/// `TYPES`: the four literal types that make the answer a foregone conclusion, named as the
/// message names them.
fn literal_kind(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    match node.kind_str() {
        // `dstr`: the parts of `"a" "b"` and the body of a heredoc are strings all the same, and
        // so is a `string` that interpolates.
        "chained_string" | "heredoc_beginning" | "string" => Some("string"),
        // `sym` and `dsym` alike: what the symbol interpolates cannot make it undefined.
        "simple_symbol" | "delimited_symbol" => Some("symbol"),
        // `?a` and the path `__FILE__` stands for are strings the grammar spells differently.
        _ => is_string(node, context).then_some("string"),
    }
}
