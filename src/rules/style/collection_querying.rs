//! `Style/CollectionQuerying`: counting a collection only to compare the count has a predicate.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};
use crate::rules::support::final_pos;

/// `REPLACEMENTS`, keyed by the comparison and the number it compares against.
const REPLACEMENTS: &[(&str, Option<&str>, &str)] = &[
    ("positive?", None, "any?"),
    (">", Some("0"), "any?"),
    ("!=", Some("0"), "any?"),
    ("zero?", None, "none?"),
    ("==", Some("0"), "none?"),
    ("==", Some("1"), "one?"),
    (">", Some("1"), "many?"),
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let active_support = context
        .setting_of::<bool>("AllCops", "ActiveSupportExtensionsEnabled")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["binary", "call"]) {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        let Some((receiver, method, argument, dot)) = comparison(node, context) else {
            continue;
        };
        let Some(count) = count_call(receiver, context) else {
            continue;
        };
        let Some((_, _, replacement)) = REPLACEMENTS.iter().find(|(name, number, _)| {
            *name == method
                && *number == argument.map(|argument| context.source.node_text(argument))
        }) else {
            continue;
        };
        // `replacement_supported?`: `many?` is an Active Support method.
        if *replacement == "many?" && !active_support {
            continue;
        }
        let Some(selector) = count.field("method") else {
            continue;
        };
        let range = selector.start_byte()..node.end_byte();
        // `removal_range`: everything from the comparison's own dot or operator, with the blank in
        // front of it.
        let removal =
            final_pos(context.source.text(), dot.start, false, false, true, false)..node.end_byte();
        offenses.push(
            context
                .offense(format!("Use `{replacement}` instead."), range)
                .corrected_by_all([
                    Edit {
                        start: selector.start_byte(),
                        end: selector.end_byte(),
                        replacement: (*replacement).to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: removal.start,
                        end: removal.end,
                        replacement: String::new(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// The comparison the cop is entered on: its receiver, the selector, the number it compares
/// against, and the range the removal starts at.
fn comparison<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(
    Node<'tree>,
    &'static str,
    Option<Node<'tree>>,
    std::ops::Range<usize>,
)> {
    match node.kind_str() {
        "binary" => {
            let operator = node.field("operator")?;
            let name = match context.source.node_text(operator) {
                ">" => ">",
                "!=" => "!=",
                "==" => "==",
                _ => return None,
            };
            let right = node.field("right")?;
            if right.kind_str() != "integer" {
                return None;
            }
            Some((
                node.field("left")?,
                name,
                Some(right),
                operator.byte_range(),
            ))
        }
        "call" => {
            let name = match context.source.node_text(node.field("method")?) {
                "positive?" => "positive?",
                "zero?" => "zero?",
                _ => return None,
            };
            if !arguments(node).is_empty() || node.field("block").is_some() {
                return None;
            }
            let dot = node.field("operator").map_or_else(
                || node.field("method").map(|name| name.byte_range()),
                |dot| Some(dot.byte_range()),
            )?;
            Some((node.field("receiver")?, name, None, dot))
        }
        _ => None,
    }
}

/// `{(any_block $(call !nil? :count) _ _) $(call !nil? :count (block-pass _)?)}`.
fn count_call<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "call" || node.field("receiver").is_none() {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "count" {
        return None;
    }
    let list = arguments(node);
    let matched = match node.field("block") {
        // The `_ _` of `(any_block ... _ _)` stands for the block's parameters and its body, and
        // `_` matches an absent one -- upstream reports `x.count {}.positive?` and
        // `x.count { it.foo? }.positive?` just as it reports `x.count { |e| e.foo? }.positive?`.
        // Requiring both fields to be present asks a stricter question than the pattern does, and
        // `_1` / `it` write no parameter list at all.
        //
        // `any_block` is also why the spelling does not matter: upstream builds `block`, `numblock`
        // and `itblock` for the three, and the pattern takes all of them.
        Some(_) => list.is_empty(),
        None => match list.as_slice() {
            [] => true,
            [only] => only.first().kind_str() == "block_argument",
            _ => false,
        },
    };
    matched.then_some(node)
}
