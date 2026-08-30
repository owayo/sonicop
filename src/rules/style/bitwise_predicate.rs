//! `Style/BitwisePredicate`: masking and comparing is what `anybits?` and its two neighbours say.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

/// `minimum_target_ruby_version 2.5`: the three predicates arrived in 2.5.
const MINIMUM: RubyVersion = RubyVersion::new(2, 5);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((receiver, method, argument)) = comparison(node, context) else {
            continue;
        };
        // `node.receiver&.begin_type?` and `bit_operation?`: the mask has to be written in
        // parentheses, which is what makes it one operand of the comparison.
        let Some((left, right)) = masked(receiver, context) else {
            continue;
        };
        let Some(preferred_method) = preferred_method(method, argument, left, right, context)
        else {
            continue;
        };
        // `allbits?` reads the flags off whichever side of the mask the comparison repeats.
        let (subject, flags) = if preferred_method == "allbits?"
            && argument.is_some_and(|argument| {
                context.source.node_text(left) == context.source.node_text(argument)
            }) {
            (right, left)
        } else {
            (left, right)
        };
        let preferred = format!(
            "{}.{preferred_method}({})",
            context.source.node_text(subject),
            context.source.node_text(flags),
        );
        offenses.push(
            context
                .offense(
                    format!("Replace with `{preferred}` for comparison with bit flags."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: preferred,
                    safe: true,
                }),
        );
    }
}

/// The comparison the cop is entered on, as its receiver, selector and lone argument.
fn comparison<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, &'static str, Option<Node<'tree>>)> {
    match node.kind_str() {
        "binary" => {
            let operator = match context.source.node_text(node.field("operator")?) {
                "!=" => "!=",
                "==" => "==",
                ">" => ">",
                ">=" => ">=",
                _ => return None,
            };
            Some((node.field("left")?, operator, Some(node.field("right")?)))
        }
        "call" => {
            let method = match context.source.node_text(node.field("method")?) {
                "positive?" => "positive?",
                "zero?" => "zero?",
                _ => return None,
            };
            if !is_plain_send(node, context)
                || !arguments(node).is_empty()
                || node.field("block").is_some()
            {
                return None;
            }
            Some((node.field("receiver")?, method, None))
        }
        _ => None,
    }
}

/// `(begin (send _ :& _))`: the two operands of a parenthesized mask.
fn masked<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if node.kind_str() != "parenthesized_statements" {
        return None;
    }
    let inner = super::nodes::children_in(node, context);
    let [inner] = inner.as_slice() else {
        return None;
    };
    if inner.kind_str() != "binary" || context.source.node_text(inner.field("operator")?) != "&" {
        return None;
    }
    Some((inner.field("left")?, inner.field("right")?))
}

/// The three matchers, in the order `preferred_method` tries them.
fn preferred_method(
    method: &str,
    argument: Option<Node<'_>>,
    left: Node<'_>,
    right: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<&'static str> {
    let integer = |wanted: &str| {
        argument.is_some_and(|argument| {
            argument.kind_str() == "integer" && context.source.node_text(argument) == wanted
        })
    };
    if method == "positive?"
        || (method == ">" && integer("0"))
        || (method == ">=" && integer("1"))
        || (method == "!=" && integer("0"))
    {
        return Some("anybits?");
    }
    // `(send (begin (send _ :& _flags)) :== _flags)`: the flags appear on both sides.
    if method == "=="
        && argument.is_some_and(|argument| {
            super::nodes::same_tree(context, right, argument)
                || super::nodes::same_tree(context, left, argument)
        })
    {
        return Some("allbits?");
    }
    if method == "zero?" || (method == "==" && integer("0")) {
        return Some("nobits?");
    }
    None
}
