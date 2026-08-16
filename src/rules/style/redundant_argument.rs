use std::collections::HashMap;

use serde::Deserialize;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal;
use crate::rules::send_node;
use crate::rules::support;

/// `NO_RECEIVER_METHODS`: the two that are written without one.
const NO_RECEIVER_METHODS: [&str; 2] = ["exit", "exit!"];

/// One entry of `Methods`: the default the method already uses.
#[derive(Deserialize)]
#[serde(untagged)]
enum MethodDefault {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl MethodDefault {
    /// `arg.inspect`, which is what the argument's own value is compared against.
    fn inspect(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Text(value) => ruby_literal::inspect_string(value),
        }
    }
}

/// An argument that only says what the method would have done anyway.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(methods) = context.setting::<HashMap<String, MethodDefault>>("Methods") else {
        return;
    };
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        // Only `exit` and `exit!` may be written without a receiver.
        if node.field("receiver").is_none() && !NO_RECEIVER_METHODS.contains(&method) {
            continue;
        }
        let Some(default) = methods.get(method) else {
            continue;
        };
        let Some(arguments) = node.field("arguments") else {
            continue;
        };
        let written = super::nodes::children(arguments);
        let [only] = written.as_slice() else {
            continue;
        };
        if written_value(*only, context) != default.inspect() {
            continue;
        }
        // `argument_range`: the parentheses go with the argument when there are any, and otherwise
        // the spaces on either side of it -- but never a line break.
        let text = context.source.text();
        let range = if context.source.node_text(arguments).starts_with('(') {
            arguments.byte_range()
        } else {
            support::final_pos(text, only.start_byte(), false, false, false, false)
                ..support::final_pos(text, only.end_byte(), true, false, false, false)
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Argument {} is redundant because it is implied by default.",
                        context.source.node_text(*only)
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `argument_matched?`: the argument as Ruby would `inspect` it, or its source when the node has no
/// value of its own -- which is what `true` and `false` fall back to.
fn written_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind_str() {
        "string" if !send_node::has_interpolation(node) => {
            ruby_literal::inspect_string(&ruby_literal::string_value(node, context))
        }
        "character" => ruby_literal::inspect_string(&ruby_literal::character_value(
            context.source.node_text(node),
        )),
        "integer" => integer_value(context.source.node_text(node)).map_or_else(
            || context.source.node_text(node).to_owned(),
            |value| value.to_string(),
        ),
        _ => context.source.node_text(node).to_owned(),
    }
}

/// `Integer#inspect`: the value in base ten, whatever base it was written in.
fn integer_value(text: &str) -> Option<i64> {
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    let (sign, digits) = match cleaned.strip_prefix('-') {
        Some(rest) => (-1, rest.to_owned()),
        None => (1, cleaned.trim_start_matches('+').to_owned()),
    };
    let lowered = digits.to_lowercase();
    let (radix, body) = if let Some(rest) = lowered.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = lowered.strip_prefix("0b") {
        (2, rest)
    } else if let Some(rest) = lowered.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = lowered.strip_prefix("0d") {
        (10, rest)
    } else if lowered.len() > 1 && lowered.starts_with('0') {
        (8, &lowered[1..])
    } else {
        (10, lowered.as_str())
    };
    i64::from_str_radix(body, radix)
        .ok()
        .map(|value| sign * value)
}
