//! The `File.open` call `Style/FileRead` and `Style/FileWrite` both start from.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, is_string, string_text};

/// The pieces both `Style/FileRead` and `Style/FileWrite` read out of a `File.open` call.
pub(super) struct Opened<'a> {
    pub filename: Range<usize>,
    pub mode: Option<&'a str>,
    pub block_pass: bool,
}

/// `(send (const {nil? cbase} :File) :open $_ (str $%MODES)? (block-pass (sym $NAME))?)`.
pub(super) fn parse<'a>(
    node: Node<'_>,
    context: &'a RuleContext<'_>,
    modes: &[&str],
    block_pass_name: &str,
) -> Option<Opened<'a>> {
    let (selector, receiver) = (node.field("method")?, node.field("receiver")?);
    if context.source.node_text(selector) != "open" || !is_plain_send(node, context) {
        return None;
    }
    if !super::nodes::is_top_level_constant(receiver, "File", context) {
        return None;
    }
    let list = arguments(node);
    let (filename, rest) = list.split_first()?;
    if filename.parts().len() > 1 {
        return None;
    }
    let mut rest = rest;
    let mut mode = None;
    if let Some(first) = rest.first()
        && is_string(first.first(), context)
    {
        let text = string_text(first.first(), context);
        if !modes.contains(&text) {
            return None;
        }
        mode = Some(text);
        rest = &rest[1..];
    }
    let block_pass = match rest {
        [] => false,
        [last] => {
            let node = last.first();
            if node.kind_str() != "block_argument" {
                return None;
            }
            let symbol = super::nodes::children(node);
            let [symbol] = symbol.as_slice() else {
                return None;
            };
            crate::rules::send_node::symbol_name(*symbol, context)? == block_pass_name
        }
        _ => return None,
    };
    Some(Opened {
        filename: filename.range(),
        mode,
        block_pass,
    })
}

/// `(block _ (args (arg $_name)) (send (lvar $_name) :NAME ...))`: the block a file was opened
/// with, when all it does is call `name` on the handle. Answers with the call's arguments.
pub(super) fn block_calls<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
    name: &str,
    arity: usize,
) -> Option<Vec<Range<usize>>> {
    let block = node.field("block")?;
    let parameters = super::nodes::children(block.field("parameters")?);
    let [parameter] = parameters.as_slice() else {
        return None;
    };
    if parameter.kind_str() != "identifier" {
        return None;
    }
    let body = super::nodes::children(block.field("body")?);
    let [statement] = body.as_slice() else {
        return None;
    };
    if statement.kind_str() != "call" || !is_plain_send(*statement, context) {
        return None;
    }
    let receiver = statement.field("receiver")?;
    if receiver.kind_str() != "identifier"
        || context.source.node_text(receiver) != context.source.node_text(*parameter)
    {
        return None;
    }
    if context.source.node_text(statement.field("method")?) != name {
        return None;
    }
    let list = arguments(*statement);
    (list.len() == arity).then(|| list.iter().map(|argument| argument.range()).collect())
}
