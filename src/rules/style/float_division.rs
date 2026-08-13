use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "single_coerce".to_owned());
    let message = match style.as_str() {
        "left_coerce" => "Prefer using `.to_f` on the left side.",
        "right_coerce" => "Prefer using `.to_f` on the right side.",
        "fdiv" => "Prefer using `fdiv` for float divisions.",
        _ => "Prefer using `.to_f` on one side only.",
    };
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((left, right)) = division(context, node) else {
            continue;
        };
        // The nth reference and `Regexp.last_match` both answer with a string, which has to be
        // converted however the style reads.
        if is_regexp_last_match(context, receiver_of(left))
            || is_regexp_last_match(context, receiver_of(right))
        {
            continue;
        }
        let (coerced_left, coerced_right) = (
            to_f_call(context, left).is_some(),
            to_f_call(context, right).is_some(),
        );
        let offending = match style.as_str() {
            "left_coerce" => coerced_right,
            "right_coerce" => coerced_left,
            "fdiv" => coerced_left || coerced_right,
            _ => coerced_left && coerced_right,
        };
        if !offending {
            continue;
        }
        let edits = match style.as_str() {
            "right_coerce" => strip(context, left)
                .into_iter()
                .chain(append(context, right))
                .collect(),
            "fdiv" => vec![fdiv(context, &locals, node, left, right)],
            _ => append(context, left)
                .into_iter()
                .chain(strip(context, right))
                .collect::<Vec<_>>(),
        };
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `(send $_ :/ $_)`: the two operands of a division, however it was written.
fn division<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    match node.kind_str() {
        "binary" => {
            let operator = node.field("operator")?;
            (context.source.node_text(operator) == "/").then_some(())?;
            Some((
                node.field("left")?,
                node.field("right")?,
            ))
        }
        _ => {
            if node.field("block").is_some()
                || !send_node::is_plain_send(node, context)
            {
                return None;
            }
            let receiver = node.field("receiver")?;
            let selector = node.field("method")?;
            if context.source.node_text(selector) != "/" {
                return None;
            }
            let arguments = node.field("arguments")?;
            match super::nodes::children(arguments).as_slice() {
                [only] => Some((receiver, *only)),
                _ => None,
            }
        }
    }
}

/// `(send !nil? :to_f)`: a `to_f` sent to something, which is what a coercion looks like.
fn to_f_call<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind_str() != "call"
        || node.field("block").is_some()
        || node.field("arguments").is_some()
        || !send_node::is_plain_send(node, context)
    {
        return None;
    }
    let receiver = node.field("receiver")?;
    let selector = node.field("method")?;
    (context.source.node_text(selector) == "to_f").then_some(receiver)
}

/// The receiver of a call, or nothing when the node is not one.
fn receiver_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    (node.kind_str() == "call")
        .then(|| node.field("receiver"))
        .flatten()
}

/// `{(send (const {nil? cbase} :Regexp) :last_match int) (:nth_ref _)}`.
fn is_regexp_last_match(context: &RuleContext<'_>, node: Option<Node<'_>>) -> bool {
    let Some(node) = node else {
        return false;
    };
    if node.kind_str() == "global_variable" {
        let name = context.source.node_text(node);
        return name.len() > 1 && name[1..].bytes().all(|byte| byte.is_ascii_digit());
    }
    if node.kind_str() != "call" {
        return false;
    }
    let (Some(receiver), Some(selector)) = (
        node.field("receiver"),
        node.field("method"),
    ) else {
        return false;
    };
    if context.source.node_text(selector) != "last_match"
        || !send_node::top_level_constant(receiver, "Regexp", context)
    {
        return false;
    }
    node.field("arguments")
        .map(super::nodes::children)
        .is_some_and(|arguments| match arguments.as_slice() {
            [only] => only.kind_str() == "integer",
            _ => false,
        })
}

/// `remove_to_f_method`: the dot and the selector go, leaving the receiver behind.
fn strip(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Edit> {
    let (Some(_), Some(dot), Some(selector)) = (
        to_f_call(context, node),
        node.field("operator"),
        node.field("method"),
    ) else {
        return Vec::new();
    };
    vec![
        Edit {
            start: dot.start_byte(),
            end: dot.end_byte(),
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: selector.start_byte(),
            end: selector.end_byte(),
            replacement: String::new(),
            safe: true,
        },
    ]
}

/// `add_to_f_method`: nothing to do where the side already coerces.
fn append(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Edit> {
    if to_f_call(context, node).is_some() {
        return Vec::new();
    }
    vec![Edit {
        start: node.end_byte(),
        end: node.end_byte(),
        replacement: ".to_f".to_owned(),
        safe: true,
    }]
}

/// `correct_from_slash_to_fdiv`: both coercions go and the division becomes the call that does it.
fn fdiv(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    left: Node<'_>,
    right: Node<'_>,
) -> Edit {
    let source = |side: Node<'_>| match to_f_call(context, side) {
        Some(receiver) => context.source.node_text(receiver).to_owned(),
        None => context.source.node_text(side).to_owned(),
    };
    let mut argument = source(right);
    // `respond_to?(:parenthesized?)`: only a call answers that, so a literal -- and a local
    // variable, which is an `lvar` rather than a call -- is written as it is.
    if is_call(locals, right) && !is_parenthesized(context, right) {
        argument = format!("({argument})");
    }
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: format!("{}.fdiv{argument}", source(left)),
        safe: true,
    }
}

/// Whether upstream's parser would have built a call here, which is what answers `parenthesized?`.
fn is_call(locals: &LocalVariables<'_, '_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "call" | "super" => true,
        "identifier" => !locals.is_lvar(node),
        _ => false,
    }
}

fn is_parenthesized(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.field("arguments")
        .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
}
