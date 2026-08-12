use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Use `Integer#times` for a simple loop which iterates a fixed number of times.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(block) = node.child_by_field_name("block") else {
            continue;
        };
        // `node.arguments.empty?`, and not a `numblock` or an `itblock`: upstream handles only
        // `on_block`, so a body reading `_1` or `it` is a node type this cop never sees.
        if block
            .child_by_field_name("parameters")
            .is_some_and(|parameters| !super::nodes::children(parameters).is_empty())
            || super::block_args::implicit(context, block)
        {
            continue;
        }
        if node
            .child_by_field_name("method")
            .is_none_or(|method| context.source.node_text(method) != "each")
        {
            continue;
        }
        let Some(range) = parenthesized_range(context, node) else {
            continue;
        };
        let (Some(low), Some(high)) = (
            integer_value(context, range.child_by_field_name("begin")),
            integer_value(context, range.child_by_field_name("end")),
        ) else {
            continue;
        };
        // `(a..b)` covers one more value than `(a...b)`.
        let inclusive = range
            .child_by_field_name("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "..");
        let Some(count) = high
            .checked_add(i64::from(inclusive))
            .and_then(|high| high.checked_sub(low))
        else {
            continue;
        };
        // `send_node.source_range`: the call up to where its block begins.
        let Some(selector_end) = block.prev_sibling().map(|previous| previous.end_byte()) else {
            continue;
        };
        let send = node.start_byte()..selector_end;
        offenses.push(context.offense(MSG, send.clone()).corrected_by(Edit {
            start: send.start,
            end: send.end,
            replacement: format!("{count}.times"),
            safe: true,
        }));
    }
}

/// `(begin ($range (int $_) (int $_)))`: the receiver has to be a parenthesized range and nothing
/// else.
fn parenthesized_range<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    let receiver = node.child_by_field_name("receiver")?;
    if receiver.kind() != "parenthesized_statements" {
        return None;
    }
    let _ = context;
    match super::nodes::children(receiver).as_slice() {
        [only] if only.kind() == "range" => Some(*only),
        _ => None,
    }
}

/// `(int _)`: the parser folds a leading sign into the literal, so `-1` is one too.
fn integer_value(context: &RuleContext<'_>, node: Option<Node<'_>>) -> Option<i64> {
    let node = node?;
    let (node, negative) = match node.kind() {
        "unary" => {
            let operator = context
                .source
                .node_text(node.child_by_field_name("operator")?);
            if !matches!(operator, "-" | "+") {
                return None;
            }
            (node.child_by_field_name("operand")?, operator == "-")
        }
        _ => (node, false),
    };
    if node.kind() != "integer" {
        return None;
    }
    let text: String = context
        .source
        .node_text(node)
        .chars()
        .filter(|character| *character != '_')
        .collect();
    let (radix, digits) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (16, &text[2..]),
        Some("0b") => (2, &text[2..]),
        Some("0o") => (8, &text[2..]),
        Some("0d") => (10, &text[2..]),
        _ if text.len() > 1 && text.starts_with('0') => (8, &text[1..]),
        _ => (10, &text[..]),
    };
    let value = i64::from_str_radix(digits, radix).ok()?;
    Some(if negative { -value } else { value })
}
