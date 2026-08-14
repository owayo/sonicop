//! `Style/ArrayIntersect`: asking whether two arrays share anything is `intersect?`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `minimum_target_ruby_version 3.1`: `Array#intersect?` arrived in 3.1.
const MINIMUM: RubyVersion = RubyVersion::new(3, 1);

/// `PREDICATES` and the two `ACTIVE_SUPPORT_PREDICATES` add.
const PREDICATES: &[&str] = &["any?", "empty?", "none?"];
const ACTIVE_SUPPORT_PREDICATES: &[&str] = &["present?", "blank?"];

/// `ARRAY_SIZE_METHODS`.
const SIZE_METHODS: &[&str] = &["count", "length", "size"];

/// `STRAIGHT_METHODS`: the ones that already read as "they share something".
const STRAIGHT_METHODS: &[&str] = &["present?", "any?", ">", "positive?", "!="];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let active_support = context
        .setting_of::<bool>("AllCops", "ActiveSupportExtensionsEnabled")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["binary", "call"]) {
        let replacement = predicate_form(node, context, active_support)
            .or_else(|| size_form(node, context))
            .or_else(|| block_form(node, context));
        let Some(replacement) = replacement else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{replacement}` instead of `{}`.",
                        context.source.node_text(node)
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `bad_intersection_check?`: `(a & b).any?` and `a.intersection(b).any?`.
fn predicate_form(
    node: Node<'_>,
    context: &RuleContext<'_>,
    active_support: bool,
) -> Option<String> {
    if node.kind_str() != "call" || node.field("block").is_some() || !arguments(node).is_empty() {
        return None;
    }
    let method = context.source.node_text(node.field("method")?);
    let allowed = PREDICATES.contains(&method)
        || (active_support && ACTIVE_SUPPORT_PREDICATES.contains(&method));
    if !allowed {
        return None;
    }
    let (left, right) = intersection(node.field("receiver")?, context)?;
    build(node, method, left, right, context)
}

/// `intersection_size_check?`: `(a & b).size > 0` and its four siblings.
fn size_form(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let (receiver, method) = match node.kind_str() {
        "binary" => {
            let operator = context.source.node_text(node.field("operator")?);
            if !matches!(operator, ">" | "!=" | "==") {
                return None;
            }
            let right = node.field("right")?;
            if right.kind_str() != "integer" || context.source.node_text(right) != "0" {
                return None;
            }
            (node.field("left")?, operator)
        }
        "call" => {
            let method = context.source.node_text(node.field("method")?);
            if !matches!(method, "positive?" | "zero?")
                || !arguments(node).is_empty()
                || node.field("block").is_some()
            {
                return None;
            }
            (node.field("receiver")?, method)
        }
        _ => return None,
    };
    // The size call is what carries the dot the replacement is written with.
    if receiver.kind_str() != "call"
        || !arguments(receiver).is_empty()
        || receiver.field("block").is_some()
    {
        return None;
    }
    if !SIZE_METHODS.contains(&context.source.node_text(receiver.field("method")?)) {
        return None;
    }
    let (left, right) = intersection(receiver.field("receiver")?, context)?;
    build(receiver, method, left, right, context)
}

/// `any_none_block_intersection`: `a.any? { |x| b.include?(x) }`.
fn block_form(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    if node.kind_str() != "call" || !arguments(node).is_empty() {
        return None;
    }
    let method = match context.source.node_text(node.field("method")?) {
        "any?" => "any?",
        "none?" => "none?",
        _ => return None,
    };
    let receiver = node.field("receiver")?;
    let block = node.field("block")?;
    let parameters = super::nodes::children(block.field("parameters")?);
    let [key] = parameters.as_slice() else {
        return None;
    };
    if key.kind_str() != "identifier" {
        return None;
    }
    let body = super::nodes::children(block.field("body")?);
    let [statement] = body.as_slice() else {
        return None;
    };
    if statement.kind_str() != "call" || statement.field("block").is_some() {
        return None;
    }
    let block_method = match context.source.node_text(statement.field("method")?) {
        "member?" => "member?",
        "include?" => "include?",
        _ => return None,
    };
    let argument = statement.field("receiver")?;
    let list = arguments(*statement);
    let [only] = list.as_slice() else {
        return None;
    };
    if context.source.node_text(only.first()) != context.source.node_text(*key) {
        return None;
    }
    let dot = node.field("operator")?;
    let dot = context.source.node_text(dot);
    // `uncorrectable_block_intersection?`.
    if method == "none?" && dot == "&." {
        return None;
    }
    if block_method == "include?"
        && !matches!(
            argument.kind_str(),
            "array" | "string_array" | "symbol_array"
        )
    {
        return None;
    }
    let bang = if method == "any?" { "" } else { "!" };
    Some(format!(
        "{bang}{}{dot}intersect?({})",
        context.source.node_text(receiver),
        context.source.node_text(argument),
    ))
}

/// `{(begin (send $_ :& $_)) (call $!nil? :intersection $_)}`: the two arrays being intersected.
fn intersection<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    match node.kind_str() {
        "parenthesized_statements" => match super::nodes::children(node).as_slice() {
            [only] if only.kind_str() == "binary" => {
                (context.source.node_text(only.field("operator")?) == "&")
                    .then(|| Some((only.field("left")?, only.field("right")?)))
                    .flatten()
            }
            _ => None,
        },
        "call" if node.field("block").is_none() => {
            if context.source.node_text(node.field("method")?) != "intersection" {
                return None;
            }
            let list = arguments(node);
            let [only] = list.as_slice() else {
                return None;
            };
            Some((node.field("receiver")?, only.first()))
        }
        _ => None,
    }
}

/// The replacement, built from the call that carries the dot.
fn build(
    dot_node: Node<'_>,
    method: &str,
    left: Node<'_>,
    right: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<String> {
    let dot = context.source.node_text(dot_node.field("operator")?);
    let bang = if STRAIGHT_METHODS.contains(&method) {
        ""
    } else {
        "!"
    };
    // A negated predicate reached through safe navigation would change what `nil` means.
    if bang == "!" && dot == "&." {
        return None;
    }
    Some(format!(
        "{bang}{}{dot}intersect?({})",
        context.source.node_text(left),
        context.source.node_text(right),
    ))
}
