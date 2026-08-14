//! `Style/FileNull`: the path of the null device has a constant.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The parents `acceptable?` lets through: a literal written as one element of something bigger is
/// data rather than a path.
const ACCEPTABLE_PARENTS: &[&str] = &["array", "pair", "chained_string"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_new_investigation`: `NUL` only names the null device on Windows, so the cop waits for
    // the file to say elsewhere that it is talking about one.
    let mut has_dev_null = false;
    for node in context.nodes_of_any(&["string", "bare_string"]) {
        if let Some(value) = string_value(node, context) {
            has_dev_null |= value.eq_ignore_ascii_case("/dev/null");
        }
    }
    for node in context.nodes_of("string") {
        let Some(value) = string_value(node, context) else {
            continue;
        };
        if context
            .parent(node)
            .is_none_or(|parent| ACCEPTABLE_PARENTS.contains(&parent.kind_str()))
        {
            continue;
        }
        if value.eq_ignore_ascii_case("nul") && !has_dev_null {
            continue;
        }
        // `%r{\A(/dev/null|NUL:?)\z}i`.
        if !(value.eq_ignore_ascii_case("/dev/null")
            || value.eq_ignore_ascii_case("nul")
            || value.eq_ignore_ascii_case("nul:"))
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Use `File::NULL` instead of `{value}`."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: "File::NULL".to_owned(),
                    safe: true,
                }),
        );
    }
}

/// The value of a `str` node, when the literal is one: a literal that interpolates is a `dstr`
/// upstream, and `valid_string?` drops an empty one and one whose bytes are not text.
fn string_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    if crate::rules::send_node::has_interpolation(node) {
        return None;
    }
    let decoded = super::literal::node_value(context, node)?;
    (decoded.valid && !decoded.value.is_empty()).then_some(decoded.value)
}
