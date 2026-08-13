//! `Style/RedundantException`: `raise RuntimeError, msg` names the class `raise msg` already picks.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{Argument, arguments, is_plain_send, send_range, top_level_constant};
use crate::rules::node_ext::NodeExt;

const MSG_1: &str = "Redundant `RuntimeError` argument can be removed.";
const MSG_2: &str = "Redundant `RuntimeError.new` call can be replaced with just the message.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let command = context.source.node_text(selector);
        if !matches!(command, "raise" | "fail")
            || node.field("receiver").is_some()
            || !is_plain_send(node, context)
        {
            continue;
        }
        let written = arguments(node);
        let range = send_range(node, context);
        // `fix_exploded` runs first and `fix_compact` only when it found nothing.
        if let Some(offense) = exploded(context, node, &written, &range, command) {
            offenses.push(offense);
            continue;
        }
        if let Some(offense) = compact(context, &written, &range) {
            offenses.push(offense);
        }
    }
}

/// `(send nil? ${:raise :fail} (const {nil? cbase} :RuntimeError) $_)`.
fn exploded(
    context: &RuleContext<'_>,
    node: Node<'_>,
    written: &[Argument<'_>],
    range: &std::ops::Range<usize>,
    command: &str,
) -> Option<Offense> {
    let [exception, message] = written else {
        return None;
    };
    if exception.parts().len() != 1
        || !top_level_constant(exception.first(), "RuntimeError", context)
    {
        return None;
    }
    if context.source.slice(message.range()) == "nil" {
        return None;
    }
    let argument = argument_source(context, message);
    let argument = if is_parenthesized(node) {
        format!("({argument})")
    } else {
        format!(" {argument}")
    };
    Some(context.offense(MSG_1, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: format!("{command}{argument}"),
        safe: true,
    }))
}

/// `(send nil? {:raise :fail} $(send (const {nil? cbase} :RuntimeError) :new $_))`.
fn compact(
    context: &RuleContext<'_>,
    written: &[Argument<'_>],
    range: &std::ops::Range<usize>,
) -> Option<Offense> {
    let [only] = written else {
        return None;
    };
    let call = only.first();
    if only.parts().len() != 1 || call.kind_str() != "call" {
        return None;
    }
    if call
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "new")
    {
        return None;
    }
    let receiver = call.field("receiver")?;
    if !top_level_constant(receiver, "RuntimeError", context) {
        return None;
    }
    let inner = arguments(call);
    let [message] = inner.as_slice() else {
        return None;
    };
    if context.source.slice(message.range()) == "nil" {
        return None;
    }
    Some(
        context
            .offense(MSG_2, range.clone())
            .corrected_by(Edit {
                start: call.start_byte(),
                end: call.end_byte(),
                replacement: argument_source(context, message),
                safe: true,
            })
            .corrections_anchored_at(call.byte_range()),
    )
}

/// `string_message?` is `any_str_type?`: anything else has to be converted first.
fn argument_source(context: &RuleContext<'_>, message: &Argument<'_>) -> String {
    let text = context.source.slice(message.range());
    let literal = message.parts().len() == 1
        && matches!(
            message.first().kind_str(),
            "string" | "chained_string" | "character" | "heredoc_beginning"
        );
    if literal {
        text.to_owned()
    } else {
        format!("{text}.to_s")
    }
}

fn is_parenthesized(node: Node<'_>) -> bool {
    node.field("arguments")
        .and_then(|list| list.child(0))
        .is_some_and(|open| open.kind_str() == "(")
}
