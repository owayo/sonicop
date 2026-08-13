//! `Style/LambdaCall`: whether a proc is invoked as `lambda.call(x)` or as `lambda.(x)`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, send_range};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let explicit = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "call");

    // `ignore_node`: a call rewritten wholesale carries the calls inside it, so the ones nested in
    // an already corrected call are still reported but no longer correctable.
    let mut corrected: Vec<std::ops::Range<usize>> = Vec::new();
    for node in context.nodes_of("call") {
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        let Some(operator) = node.field("operator") else {
            continue;
        };
        let selector = node.field("method");
        // `RESTRICT_ON_SEND = %i[call]`: `foo.()` is a call to `:call` with no selector written.
        let implicit = selector.is_none();
        if !implicit && selector.is_some_and(|name| context.source.node_text(name) != "call") {
            continue;
        }
        if explicit != implicit {
            continue;
        }
        let range = send_range(node, context);
        // Rebuilding the call as one expression would drop any comment inside it.
        if context
            .comment_ranges()
            .iter()
            .any(|comment| range.start <= comment.start && comment.end <= range.end)
        {
            continue;
        }
        let written: Vec<&str> = arguments(node)
            .iter()
            .map(|argument| context.source.slice(argument.range()))
            .collect();
        let call_arguments = if written.is_empty() {
            String::new()
        } else {
            format!("({})", written.join(", "))
        };
        let prefer = format!(
            "{}{}{}",
            context.source.node_text(receiver),
            context.source.node_text(operator),
            if explicit {
                format!("call{call_arguments}")
            } else {
                format!("({})", written.join(", "))
            }
        );
        let message = format!(
            "Prefer the use of `{prefer}` over `{}`.",
            context.source.slice(range.clone())
        );
        let offense = context.offense(message, range.clone());
        if corrected
            .iter()
            .any(|outer| outer.start <= range.start && range.end <= outer.end)
        {
            offenses.push(offense);
            continue;
        }
        corrected.push(range.clone());
        offenses.push(offense.corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: prefer,
            safe: true,
        }));
    }
}
