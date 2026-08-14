//! `Style/FileEmpty`: sizing or reading a file whole only to ask whether it holds anything.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, send_range};

/// `minimum_target_ruby_version 2.4`: `File.empty?` arrived in 2.4.
const MINIMUM: RubyVersion = RubyVersion::new(2, 4);

/// The two constants that answer `empty?`, as `{:File :FileTest}` names them.
const FILE_CLASSES: &[&str] = &["File", "FileTest"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((constant, argument)) = matched(node, context) else {
            continue;
        };
        let class = context.source.node_text(constant);
        let argument = context.source.slice(argument);
        let range = send_range(node, context);
        // The message names the plain call even where the correction negates it: upstream's
        // `format(MSG, ...)` leaves `bang` out of it.
        offenses.push(
            context
                .offense(
                    format!("Use `{class}.empty?({argument})` instead."),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: format!("{}{class}.empty?({argument})", bang(node, context)),
                    safe: true,
                }),
        );
    }
}

/// `bang`: `!` for the comparisons that read as "not empty".
///
/// Upstream asks the *receiver* of the outer call whether it is a `!`, which is how the two
/// `(send (send ... :!) ...)` patterns flip back.
fn bang(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    if node.kind_str() != "binary" {
        return "";
    }
    let Some(operator) = node.field("operator") else {
        return "";
    };
    let operator = context.source.node_text(operator);
    let negated_operand = node.field("left").is_some_and(is_negation);
    let flip = (operator == "==" && negated_operand)
        || (matches!(operator, ">=" | "!=") && !negated_operand);
    if flip { "!" } else { "" }
}

/// The seven shapes upstream's `offensive?` matches, answering with the constant and the path.
fn matched<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Range<usize>)> {
    let chained = match node.kind_str() {
        "binary" => comparison(node, context),
        "call" => predicate(node, context),
        _ => None,
    };
    // `File.zero?(path)` is the one shape that is a call of the constant itself.
    chained.or_else(|| {
        if node.kind_str() != "call" {
            return None;
        }
        file_call(node, &["zero?"], context)
    })
}

/// The four shapes written as a comparison.
fn comparison<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Range<usize>)> {
    let operator = context.source.node_text(node.field("operator")?);
    let mut left = node.field("left")?;
    // `(send (send ... :!) ...)`: the two patterns that look through a negation.
    if is_negation(left) {
        left = left.field("operand")?;
    }
    let right = node.field("right")?;
    match operator {
        // `File.size(path) {== >=} 0`.
        "==" | ">=" if right.kind_str() == "integer" => {
            if context.source.node_text(right) != "0" {
                return None;
            }
            file_call(left, &["size"], context)
        }
        // `File.read(path) {== !=} ''`.
        "==" | "!=" if is_empty_string(right) => file_call(left, &["read", "binread"], context),
        _ => None,
    }
}

/// The two shapes written as a predicate on the result.
fn predicate<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Range<usize>)> {
    if !is_plain_send(node, context) || !arguments(node).is_empty() {
        return None;
    }
    let receiver = node.field("receiver")?;
    match context.source.node_text(node.field("method")?) {
        // `File.size(path).zero?`.
        "zero?" => file_call(receiver, &["size"], context),
        // `File.read(path).empty?`.
        "empty?" => file_call(receiver, &["read", "binread"], context),
        _ => None,
    }
}

/// `(send $(const {nil? cbase} {:File :FileTest}) $NAMES $_)`: a one-argument call on either
/// constant.
fn file_call<'tree>(
    node: Node<'tree>,
    names: &[&str],
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Range<usize>)> {
    if node.kind_str() != "call" || node.field("block").is_some() || !is_plain_send(node, context) {
        return None;
    }
    if !names.contains(&context.source.node_text(node.field("method")?)) {
        return None;
    }
    let receiver = node.field("receiver")?;
    if !FILE_CLASSES
        .iter()
        .any(|class| super::nodes::is_top_level_constant(receiver, class, context))
    {
        return None;
    }
    let list = arguments(node);
    let [argument] = list.as_slice() else {
        return None;
    };
    Some((receiver, argument.range()))
}

/// `(send _ :!)`: upstream's parser spells `!x` as a call, and the two patterns that look through
/// one ask for exactly that.
fn is_negation(node: Node<'_>) -> bool {
    node.kind_str() == "unary"
        && node
            .child(0)
            .is_some_and(|operator| operator.kind_str() == "!")
}

/// `(str empty?)`: a plain string literal holding nothing.
fn is_empty_string(node: Node<'_>) -> bool {
    node.kind_str() == "string" && node.named_child_count() == 0
}
