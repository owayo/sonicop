use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `(send (const {nil? cbase} :Regexp) {:new :compile} (regexp $... (regopt $...)))`.
///
/// The replacement is rebuilt from the parts rather than copied from the argument, which is why a
/// `%r{...}` written inside the constructor comes back out as `/.../`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method"))
        else {
            continue;
        };
        if !send_node::top_level_constant(receiver, "Regexp", context) {
            continue;
        }
        let method = context.source.node_text(selector);
        if method != "new" && method != "compile" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let [only] = arguments.as_slice() else {
            continue;
        };
        let Some((pattern, options)) = literal_parts(*only, context) else {
            continue;
        };
        // Upstream's `send` node stops where its arguments do, so a block written after the call
        // stays outside both the report and the replacement.
        let range = send_node::send_range(node, context);
        offenses.push(
            context
                .offense(
                    format!("Remove the redundant `Regexp.{method}`."),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: format!("/{pattern}/{options}"),
                    safe: true,
                }),
        );
    }
}

/// The body and the option letters of a regexp literal.
///
/// Upstream reads them off the `regexp` node's children and its trailing `regopt`. Here the
/// delimiters are the first and last children of the `regex` node, and the closing one carries the
/// options after it -- `/im` for `/x/im`, `}im` for `%r{x}im`.
fn literal_parts(node: Node<'_>, context: &RuleContext<'_>) -> Option<(String, String)> {
    if node.kind_str() != "regex" {
        return None;
    }
    let opening = node.child(0)?;
    let closing = node.child(u32::try_from(node.child_count().checked_sub(1)?).ok()?)?;
    if opening.id() == closing.id() {
        return None;
    }
    let pattern = context.source.text()[opening.end_byte()..closing.start_byte()].to_owned();
    let closing_text = context.source.node_text(closing);
    let options = closing_text
        .char_indices()
        .nth(1)
        .map_or_else(String::new, |(offset, _)| closing_text[offset..].to_owned());
    Some((pattern, options))
}
