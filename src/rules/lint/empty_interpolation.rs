use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::named_children;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Empty interpolation detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("interpolation") {
        if in_percent_literal_array(node) || !interpolates_nothing(node) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// `in_percent_literal_array?`: the nearest enclosing array, when there is one, was written as a
/// percent literal. `%W[#{}]` produces an empty element rather than an empty interpolation, so
/// removing the interpolation would drop the element.
fn in_percent_literal_array(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(current) = parent {
        match current.kind_str() {
            "array" => return false,
            "string_array" | "symbol_array" => return true,
            _ => parent = current.parent(),
        }
    }
    false
}

/// Whether every child of the interpolation contributes nothing: upstream drops the `nil`s and the
/// empty string literals before asking whether anything is left.
fn interpolates_nothing(node: Node<'_>) -> bool {
    named_children(node).into_iter().all(|child| {
        match child.kind_str() {
            // A `;` is a separator upstream rather than a child of the `begin` it stands in.
            "empty_statement" | "comment" => true,
            "nil" => true,
            // `basic_literal? && str_content.empty?`: only a string literal written with nothing
            // between its delimiters. One holding an interpolation or an escape is a `dstr` there.
            "string" => named_children(child).is_empty(),
            _ => false,
        }
    })
}
