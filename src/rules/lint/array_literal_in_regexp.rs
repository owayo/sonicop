use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{has_interpolation, string_text, symbol_name};

use super::node_equality::numeric_value;
use crate::rules::send_node::named_children_of;

const MSG_CHARACTER_CLASS: &str =
    "Use a character class instead of interpolating an array in a regexp.";
const MSG_ALTERNATION: &str = "Use alternation instead of interpolating an array in a regexp.";
const MSG_UNKNOWN: &str =
    "Use alternation or a character class instead of interpolating an array in a regexp.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("interpolation") {
        if node
            .parent_of(context)
            .is_none_or(|parent| parent.kind_str() != "regex")
        {
            continue;
        }
        let Some(array) = named_children_of(node, context)
            .into_iter()
            .rfind(|child| child.kind_str() != "comment")
            // `%w[…]` and `%i[…]` are `array` nodes upstream. The grammar gives each percent form
            // a kind of its own, so asking for `array` alone missed every one of them.
            .filter(|last| matches!(last.kind_str(), "array" | "string_array" | "symbol_array"))
        else {
            continue;
        };
        let elements: Vec<Node<'_>> = named_children_of(array, context)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect();
        let Some(values) = elements
            .iter()
            .map(|element| literal_value(*element, context))
            .collect::<Option<Vec<String>>>()
        else {
            offenses.push(context.offense(MSG_UNKNOWN, node.byte_range()));
            continue;
        };
        // `character_class?`: every value is one character long, so they fit between brackets.
        let escaped: Vec<String> = values.iter().map(|value| escape(value)).collect();
        let (message, replacement) = if values.iter().all(|value| value.chars().count() == 1) {
            (MSG_CHARACTER_CLASS, format!("[{}]", escaped.join("")))
        } else {
            (MSG_ALTERNATION, format!("(?:{})", escaped.join("|")))
        };
        let range = node.byte_range();
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement,
            safe: true,
        }));
    }
}

/// `array_values`: what the element would print as, or `None` for anything that is not one of the
/// literal types the cop can rewrite.
fn literal_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "integer" => numeric_value(node, context).map(|value| format!("{}", value as i64)),
        "unary" => {
            let operand = node.field("operand")?;
            matches!(operand.kind_str(), "integer" | "float")
                .then(|| {
                    if operand.kind_str() == "integer" {
                        numeric_value(node, context).map(|value| format!("{}", value as i64))
                    } else {
                        Some(context.source.node_text(node).to_owned())
                    }
                })
                .flatten()
        }
        "float" => Some(context.source.node_text(node).to_owned()),
        // A `%w[…]` element is a `str` upstream like any other; the grammar spells it
        // `bare_string`, and a `%i[…]` element `bare_symbol`.
        "string" | "bare_string" if !has_interpolation(node) => {
            Some(string_text(node, context).to_owned())
        }
        "bare_symbol" => Some(string_text(node, context).to_owned()),
        "character" => Some(string_text(node, context).to_owned()),
        "simple_symbol" | "delimited_symbol" | "hash_key_symbol" => {
            symbol_name(node, context).map(str::to_owned)
        }
        // `true`, `false` and `nil` have no value of their own, so the source is what is used.
        "true" | "false" | "nil" => Some(context.source.node_text(node).to_owned()),
        _ => None,
    }
}

/// `Regexp.escape`: the metacharacters and the whitespace it spells out.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '[' | ']' | '{' | '}' | '(' | ')' | '|' | '-' | '*' | '.' | '\\' | '?' | '+' | '^'
            | '$' | '#' | ' ' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            '\u{c}' => escaped.push_str("\\f"),
            '\u{b}' => escaped.push_str("\\v"),
            _ => escaped.push(character),
        }
    }
    escaped
}
