//! `Style/RaiseArgs`: `raise Klass, message` rather than `raise Klass.new(message)`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range};
use crate::rules::node_ext::NodeExt;

/// `ACCEPTABLE_ARG_TYPES`: an argument that `new` needs and `raise` cannot pass on.
const ACCEPTABLE_ARG_KINDS: &[&str] = &[
    "pair",
    "hash",
    "hash_splat_argument",
    "splat_argument",
    "forward_argument",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let compact = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "compact");
    let allowed: Vec<String> = context.setting("AllowedCompactTypes").unwrap_or_default();

    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        // `node.command?`: the keyword is only itself when it is called without a receiver.
        if !matches!(method, "raise" | "fail")
            || node.field("receiver").is_some()
            || !is_plain_send(node, context)
        {
            continue;
        }
        let written = arguments(node);
        let range = send_range(node, context);
        let offense = if compact {
            if !reports_compact(&written) {
                continue;
            }
            let offense = context.offense(
                format!("Provide an exception object as an argument to `{method}`."),
                range.clone(),
            );
            // `correction_exploded_to_compact` hands back the original source when there is more
            // than one message argument, so the offense stands without a rewrite.
            match exploded_to_compact(context, node, &written, &range) {
                Some(edit) => offense.corrected_by(edit),
                None => offense,
            }
        } else {
            let Some(edit) = compact_to_exploded(context, node, &written, &range, &allowed) else {
                continue;
            };
            context
                .offense(
                    format!("Provide an exception class and message as arguments to `{method}`."),
                    range.clone(),
                )
                .corrected_by(edit)
        };
        offenses.push(offense);
    }
}

/// `check_exploded` plus `correction_compact_to_exploded`.
fn compact_to_exploded(
    context: &RuleContext<'_>,
    node: Node<'_>,
    written: &[crate::rules::send_node::Argument<'_>],
    range: &std::ops::Range<usize>,
    allowed: &[String],
) -> Option<Edit> {
    let [only] = written else {
        return None;
    };
    let first = only.first();
    // `use_new_method?`: a call to `new` on something.
    if first.kind_str() != "call" || only.parts().len() != 1 {
        return None;
    }
    let receiver = first.field("receiver")?;
    if first
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "new")
    {
        return None;
    }
    let inner = arguments(first);
    // `acceptable_exploded_args?`: more than one argument, or a single one `raise` cannot forward
    // on its own. `new` with no arguments at all is *not* acceptable -- `raise Klass` says the same.
    if inner.len() > 1
        || inner
            .first()
            .is_some_and(|argument| ACCEPTABLE_ARG_KINDS.contains(&argument.first().kind_str()))
    {
        return None;
    }
    // `allowed_non_exploded_type?`.
    if allowed
        .iter()
        .any(|name| name == context.source.node_text(receiver))
    {
        return None;
    }
    let message = inner
        .first()
        .map(|argument| context.source.slice(argument.range()));
    let mut parts = vec![context.source.node_text(receiver)];
    parts.extend(message);
    Some(Edit {
        start: range.start,
        end: range.end,
        replacement: assemble(context, node, &parts.join(", ")),
        safe: true,
    })
}

/// `check_compact` plus `correction_exploded_to_compact`.
/// `check_compact`: more than one argument, unless the exception itself was handed a hash.
fn reports_compact(written: &[crate::rules::send_node::Argument<'_>]) -> bool {
    if written.len() <= 1 {
        return false;
    }
    let exception = written[0].first();
    // A call whose own first argument is a hash is `raise Klass, key: value`, which stays.
    !(exception.kind_str() == "call"
        && arguments(exception)
            .first()
            .is_some_and(|argument| matches!(argument.first().kind_str(), "pair" | "hash")))
}

fn exploded_to_compact(
    context: &RuleContext<'_>,
    node: Node<'_>,
    written: &[crate::rules::send_node::Argument<'_>],
    range: &std::ops::Range<usize>,
) -> Option<Edit> {
    if !reports_compact(written) || written.len() > 2 {
        return None;
    }
    let exception = written[0].first();
    let argument = context.source.slice(written[1].range());
    let exception_class = exception.field("receiver").map_or_else(
        || context.source.node_text(exception),
        |receiver| context.source.node_text(receiver),
    );
    Some(Edit {
        start: range.start,
        end: range.end,
        replacement: assemble(context, node, &format!("{exception_class}.new({argument})")),
        safe: true,
    })
}

/// The rebuilt call, parenthesized where the keyword binds looser than what surrounds it.
fn assemble(context: &RuleContext<'_>, node: Node<'_>, arguments: &str) -> String {
    let Some(selector) = node.field("method") else {
        return arguments.to_owned();
    };
    let method = context.source.node_text(selector);
    if requires_parentheses(context, node) {
        format!("{method}({arguments})")
    } else {
        format!("{method} {arguments}")
    }
}

/// `requires_parens?`: an operand of `and`/`or`, or part of a ternary. `operator_keyword?` is
/// `type?(:and, :or)`, which `&&` and `||` build just as the keywords do.
fn requires_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    match parent.kind_str() {
        "binary" => parent
            .field("operator")
            .is_some_and(|operator| {
                matches!(
                    context.source.node_text(operator),
                    "and" | "or" | "&&" | "||"
                )
            }),
        "conditional" => true,
        _ => false,
    }
}
