use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// The node kinds a `sym` reaches tree-sitter as. A `%i[]` element is a `bare_symbol` instead,
/// which is how the percent literal upstream exempts stays exempt.
const SYMBOL_KINDS: &[&str] = &[
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    // `{ "true": 1 }` writes a symbol key with string quotes around it.
    "string",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(SYMBOL_KINDS) {
        let Some(boolean) = boolean_name(node, context) else {
            continue;
        };
        let message =
            format!("Symbol with a boolean name - you probably meant to use `{boolean}`.");
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by_all(corrections(node, context)),
        );
    }
}

/// The name of the symbol, when it is one of the two booleans written as one.
fn boolean_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let text = context.source.node_text(node);
    let name = match node.kind() {
        "simple_symbol" => text.strip_prefix(':')?,
        "hash_key_symbol" => text,
        // A symbol whose quotes hold an interpolation is a `dsym`, which is a different node
        // upstream and no literal name at all.
        "delimited_symbol" => quoted_content(node, context)?,
        "string" => {
            let pair = node.parent().filter(|parent| parent.kind() == "pair")?;
            let key = pair.child_by_field_name("key")?;
            if key.id() != node.id() || !colon_pair(pair) {
                return None;
            }
            quoted_content(node, context)?
        }
        _ => return None,
    };
    matches!(name, "true" | "false").then_some(name)
}

/// The text between the quotes, when that is all there is between them.
fn quoted_content<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    let parts: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
    match parts.as_slice() {
        [content] if content.kind() == "string_content" => Some(context.source.node_text(*content)),
        _ => None,
    }
}

/// `PairNode#colon?`: whether the pair was written `key: value` rather than with a hash rocket.
fn colon_pair(pair: Node<'_>) -> bool {
    separator(pair).is_some_and(|separator| separator.kind() == ":")
}

fn separator(pair: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = pair.walk();
    let separator = pair
        .children(&mut cursor)
        .find(|child| !child.is_named() && matches!(child.kind(), ":" | "=>"))?;
    Some(separator)
}

/// Dropping the colon turns the symbol into the boolean it was named after. A key written
/// `true:` has its colon on the other side, so the pair is rewritten with a hash rocket instead --
/// `true:` is not a key Ruby will parse.
fn corrections(node: Node<'_>, context: &RuleContext<'_>) -> Vec<Edit> {
    let source = context.source.node_text(node);
    let key_of = node
        .parent()
        .filter(|parent| parent.kind() == "pair" && colon_pair(*parent))
        .filter(|parent| {
            parent
                .child_by_field_name("key")
                .is_some_and(|key| key.id() == node.id())
        });
    let Some(colon) = key_of.and_then(separator) else {
        return vec![Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: source.replace(':', ""),
            safe: true,
        }];
    };
    vec![
        Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: format!("{source} =>"),
            safe: true,
        },
        Edit {
            start: colon.start_byte(),
            end: colon.end_byte(),
            replacement: String::new(),
            safe: true,
        },
    ]
}
