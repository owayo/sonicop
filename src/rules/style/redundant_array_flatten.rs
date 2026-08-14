use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Remove the redundant `flatten`.";

/// `(call (call !nil? :flatten _?) :join (nil)?)` matched against the parent of the `flatten` call.
///
/// `Array#join` flattens on its own, so the call in front of it does nothing. The pattern only
/// accepts a `join` written without arguments or with a literal `nil`, because any other separator
/// would change what a nested array joins to.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "flatten" {
            continue;
        }
        // `!nil?`: the receiver has to be written out, so a bare `flatten` is not a match.
        if node.field("receiver").is_none() {
            continue;
        }
        // A block turns the call into a `block` node upstream, which the inner `(call ...)` of the
        // pattern never matches.
        if node.field("block").is_some() {
            continue;
        }
        // `_?`: `flatten` itself may take a depth argument, but no more than one.
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        if arguments.len() > 1 {
            continue;
        }
        if !is_plain_join(node, context) {
            continue;
        }
        // `node.loc.dot.begin.join(node.source_range.end)`: the dot through the end of the call, so
        // removing it leaves the receiver joined straight to `join`.
        let Some(operator) = node.field("operator") else {
            continue;
        };
        let range = operator.start_byte()..node.end_byte();
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// `(call <flatten> :join (nil)?)`, with the `flatten` call as the receiver rather than an argument.
fn is_plain_join(flatten: tree_sitter::Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = flatten.parent() else {
        return false;
    };
    if parent.kind_str() != "call" {
        return false;
    }
    if parent.field("receiver").map(|node| node.id()) != Some(flatten.id()) {
        return false;
    }
    if parent
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "join")
    {
        return false;
    }
    let arguments = parent
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    match arguments.as_slice() {
        [] => true,
        [only] => only.kind_str() == "nil",
        _ => false,
    }
}
