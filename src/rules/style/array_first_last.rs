use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `arr[0]` and `arr[-1]`, which `first` and `last` say better.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((receiver, index, selector_start)) = subscript(node, context) else {
            continue;
        };
        // `node.arguments.size == 1 && node.first_argument.int_type?`, then `[0, -1]`.
        let preferred = match context.source.node_text(index) {
            "0" => "first",
            "-1" => "last",
            _ => continue,
        };
        // `innermost_braces_node`: a chain of subscripts is reported from its innermost link.
        let mut innermost = (node, receiver, selector_start);
        while let Some(inner) = subscript(innermost.1, context) {
            innermost = (innermost.1, inner.0, inner.2);
        }
        let (node, _, selector_start) = innermost;
        // `brace_method?(node.parent)`: a subscript of a subscript is left to the outer one.
        if enclosing_subscript(node) {
            continue;
        }
        // `compound_assignment_target?`, and the `[]=` spelling, which is a different selector
        // upstream and never reaches the cop.
        if is_assignment_target(node) {
            continue;
        }
        // `find_offense_range`: with a dot the selector runs to the end of the call, without one it
        // is the `[...]` itself -- and the replacement grows a dot to match.
        let (range, replacement) = match node.kind_str() {
            "element_reference" => (selector_start..node.end_byte(), format!(".{preferred}")),
            _ => (
                selector_start..send_node::send_range(node, context).end,
                preferred.to_owned(),
            ),
        };
        offenses.push(
            context
                .offense(format!("Use `{preferred}`."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The receiver, the single subscript and where the selector begins, for both spellings of `[]`.
fn subscript<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>, usize)> {
    match node.kind_str() {
        "element_reference" => {
            let object = node.field("object")?;
            let children = super::nodes::children(node);
            let [_, index] = children.as_slice() else {
                return None;
            };
            let bracket = context.source.text()[object.end_byte()..node.end_byte()].find('[')?;
            Some((object, *index, object.end_byte() + bracket))
        }
        "call" => {
            let selector = node.field("method")?;
            if context.source.node_text(selector) != "[]" {
                return None;
            }
            let receiver = node.field("receiver")?;
            let arguments = super::nodes::children(node.field("arguments")?);
            let [index] = arguments.as_slice() else {
                return None;
            };
            Some((receiver, *index, selector.start_byte()))
        }
        _ => None,
    }
}

/// Whether the node is the receiver of another subscript, in either direction of the chain.
fn enclosing_subscript(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    // `brace_method?` asks only what the parent is, so a subscript written as another one's index
    // (`a[b[0]]`) is left alone just as one written as its receiver is.
    parent.kind_str() == "element_reference"
}

/// Whether the subscript is what an assignment writes to, which upstream spells `[]=` or wraps in
/// an `op_asgn`.
fn is_assignment_target(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(parent.kind_str(), "assignment" | "operator_assignment")
            && parent
                .field("left")
                .is_some_and(|target| target.id() == node.id())
    })
}
