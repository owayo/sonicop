//! `Style/ExactRegexpMatch`: a regexp anchored at both ends only asks whether two strings are equal.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// The characters `regexp_parser` reads as something other than one literal run, which is what
/// `tokens == [:bos, :literal, :eos]` asks the pattern to be.
const METACHARACTERS: &[char] = &[
    '.', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '^', '$', '\\',
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((receiver, method, regexp)) = match_call(node, context) else {
            continue;
        };
        let Some(text) = exact_match_text(regexp, context) else {
            continue;
        };
        // `escape_single_quotes`: the literal is written back inside single quotes.
        let quoted = text.replace('\\', "\\\\").replace('\'', "\\'");
        let replacement = format!(
            "{} {} '{quoted}'",
            context.source.node_text(receiver),
            if method == "!~" { "!=" } else { "==" },
        );
        offenses.push(
            context
                .offense(format!("Use `{replacement}`."), node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `(call _ {:=~ :=== :!~ :match :match?} (regexp (str $_) (regopt)))`, with the receiver required
/// by `return unless (receiver = node.receiver)`.
fn match_call<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, &'static str, Node<'tree>)> {
    match node.kind_str() {
        "binary" => {
            let operator = match context.source.node_text(node.field("operator")?) {
                "=~" => "=~",
                "===" => "===",
                "!~" => "!~",
                _ => return None,
            };
            let right = node.field("right")?;
            (right.kind_str() == "regex").then_some((node.field("left")?, operator, right))
        }
        "call" => {
            let method = match context.source.node_text(node.field("method")?) {
                "match" => "match",
                "match?" => "match?",
                _ => return None,
            };
            let receiver = node.field("receiver")?;
            let list = arguments(node);
            let [argument] = list.as_slice() else {
                return None;
            };
            let regexp = argument.first();
            (regexp.kind_str() == "regex").then_some((receiver, method, regexp))
        }
        _ => None,
    }
}

/// The literal a `/\A...\z/` spells out, when that is all the pattern is.
///
/// `exact_match_pattern?` reads the parsed regexp as exactly three tokens -- the start anchor, one
/// unquantified literal, and the end anchor. The grammar already splits the anchors out as escape
/// sequences, so what is left is checking that the run between them holds nothing a regexp engine
/// would read as syntax, and that no flag was written after the closing delimiter.
fn exact_match_text<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let parts = super::nodes::children(node);
    let [start, literal, end] = parts.as_slice() else {
        return None;
    };
    if context.source.node_text(*start) != "\\A"
        || context.source.node_text(*end) != "\\z"
        || literal.kind_str() != "string_content"
    {
        return None;
    }
    // `(regopt)` with nothing in it: only the closing delimiter may follow the last part.
    if node.end_byte() - end.end_byte() != 1 {
        return None;
    }
    let text = context.source.node_text(*literal);
    (!text.contains(METACHARACTERS)).then_some(text)
}
