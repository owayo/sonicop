//! `EndlessMethodRewriter`: the half `Style/EndlessMethod` and
//! `Style/AmbiguousEndlessMethodDefinition` share.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `node.endless?`: upstream reads `loc.assignment`, which is the `=` the grammar writes as an
/// anonymous child of the definition.
pub(super) fn is_endless(node: Node<'_>) -> bool {
    super::conditional::token(node, &["="]).is_some()
}

/// `correct_to_multiline`: the same definition written out over three lines.
pub(super) fn correct_to_multiline(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let body = node.field("body")?;
    let column = node.start_position().column;
    Some(format!(
        "def {}{}{}\n{}{}\n{}end",
        receiver(context, node),
        context.source.node_text(node.field("name")?),
        arguments(context, node),
        " ".repeat(column + indentation_width(context)),
        context.source.node_text(body),
        " ".repeat(column),
    ))
}

/// `receiver`: the `self.` of a singleton definition, and nothing for a plain one.
pub(super) fn receiver(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let Some(object) = node.field("object") else {
        return String::new();
    };
    let operator = super::conditional::token(node, &[".", "::", "&."])
        .map_or(".", |operator| context.source.node_text(operator));
    format!("{}{operator}", context.source.node_text(object))
}

/// `arguments`: `node.arguments.any? ? node.arguments.source : ''`.
///
/// Upstream asks the list what it holds, not whether it was written, so an empty `()` is dropped:
/// `def foo() = x` rewrites to `def foo`, not `def foo()`.
pub(super) fn arguments<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> &'a str {
    let Some(parameters) = node.field("parameters") else {
        return "";
    };
    match super::nodes::children_in(parameters, context).is_empty() {
        true => "",
        false => context.source.node_text(parameters),
    }
}

/// `configured_indentation_width`.
pub(super) fn indentation_width(context: &RuleContext<'_>) -> usize {
    context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2)
        .max(0) as usize
}
