use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const PREFER_EACH: &str = "Prefer `each` over `for`.";
const PREFER_FOR: &str = "Prefer `for` over `each`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "each".to_owned());
    if style == "each" {
        for node in context.nodes_of("for") {
            let Some(offense) = for_to_each(context, node) else {
                continue;
            };
            offenses.push(offense);
        }
        return;
    }
    for node in context.nodes_of("call") {
        let Some(offense) = each_to_for(context, node) else {
            continue;
        };
        offenses.push(offense);
    }
}

/// `ForToEachCorrector`: the head of the loop becomes the receiver, the call and the block's
/// parameter list.
fn for_to_each(context: &RuleContext<'_>, node: Node<'_>) -> Option<Offense> {
    let variable = node.field("pattern")?;
    let value = node.field("value")?;
    let collection = *super::nodes::children(value).first()?;
    let body = node.field("body")?;
    // `for_node.do?`: the head ends at the `do` when there is one, and at the collection otherwise.
    let end = match body.child(0).filter(|first| first.kind_str() == "do") {
        Some(keyword) => keyword.end_byte(),
        None => collection.end_byte(),
    };
    let dot = match is_safe_navigation(context, collection) {
        true => "&.",
        false => ".",
    };
    let source = context.source.node_text(collection);
    let collection_source = match requires_parentheses(context, collection) {
        true => format!("({source})"),
        false => source.to_owned(),
    };
    Some(
        context
            .offense(PREFER_EACH, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end,
                replacement: format!(
                    "{collection_source}{dot}each do |{}|",
                    context.source.node_text(variable)
                ),
                safe: true,
            }),
    )
}

/// `EachToForCorrector`: the receiver and the block's parameters become the head of a `for`.
fn each_to_for(context: &RuleContext<'_>, node: Node<'_>) -> Option<Offense> {
    let block = node.field("block")?;
    // `suspect_enumerable?`, and `return unless node.receiver`.
    let receiver = node.field("receiver")?;
    let selector = node.field("method")?;
    if context.source.node_text(selector) != "each"
        || node.field("arguments").is_some()
    {
        return None;
    }
    let range = node.byte_range();
    if context.source.line_column(range.start).0 == context.source.line_column(range.end).0 {
        return None;
    }
    let parameters = block.field("parameters");
    let written = parameters.map(super::nodes::children).unwrap_or_default();
    let (end, correction) = match written.is_empty() {
        // An empty `| |` is still no parameter at all, so the head stops at the block's opening.
        true => (
            block.child(0)?.end_byte(),
            format!("for _ in {} do", context.source.node_text(receiver)),
        ),
        false => (
            parameters?.end_byte(),
            format!(
                "for {} in {} do",
                written
                    .iter()
                    .map(|parameter| context.source.node_text(*parameter))
                    .collect::<Vec<_>>()
                    .join(", "),
                context.source.node_text(receiver)
            ),
        ),
    };
    Some(
        context
            .offense(PREFER_FOR, range.clone())
            .corrected_by(Edit {
                start: range.start,
                end,
                replacement: correction,
                safe: true,
            }),
    )
}

fn is_safe_navigation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "call"
        && node
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "&.")
}

/// `requires_parentheses?`: an operator call, a range and a `and`/`or` all bind looser than the
/// `.each` written after them.
fn requires_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "range" => true,
        "binary" => node
            .field("operator")
            .is_some_and(|operator| {
                let text = context.source.node_text(operator);
                super::nodes::is_operator_method(text) || matches!(text, "and" | "or" | "&&" | "||")
            }),
        "unary" => node
            .field("operator")
            .is_some_and(|operator| {
                super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        // `a[0]` is a call to `:[]`, which is an operator method too.
        "element_reference" => true,
        "call" => node
            .field("method")
            .is_some_and(|method| method.kind_str() == "operator"),
        _ => false,
    }
}
