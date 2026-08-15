use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Remove the redundant current directory path.";

/// `RESTRICT_ON_SEND = %i[require_relative]`: the receiver is never checked, only the name.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "require_relative" {
            continue;
        }
        let Some(argument) = node
            .field("arguments")
            .and_then(|arguments| super::nodes::children(arguments).into_iter().next())
        else {
            continue;
        };
        // `first_argument.source.index(%r{\./+})`: the offset is taken in the **source**, so the
        // opening quote and a `%q{` prefix are counted in.
        let Some(index) = current_directory_index(context.source.node_text(argument)) else {
            continue;
        };
        let Some(content) = leading_path_content(argument, context) else {
            continue;
        };
        let Some(length) = redundant_path_length(content) else {
            continue;
        };
        let start = argument.start_byte() + index;
        let range = start..start + length;
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// The first `\./+` in the text: a dot followed by at least one slash.
fn current_directory_index(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    (0..bytes.len()).find(|index| bytes[*index] == b'.' && bytes.get(index + 1) == Some(&b'/'))
}

/// `leading_path_content`: the string value for a `str`, and for a `dstr` the value of its first
/// child when that child is a `str`. Anything else -- an expression, or a literal that interpolates
/// right away -- has no leading path to trim.
fn leading_path_content<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if node.kind_str() != "string" {
        return None;
    }
    let first = super::nodes::children(node).into_iter().next()?;
    if first.kind_str() != "string_content" {
        return None;
    }
    Some(context.source.node_text(first))
}

/// `redundant_path_length`: the length of a `\A\./+` prefix, which is what a leading `./` or `.//`
/// contributes. A `./` anywhere else in the path means something and is left alone.
fn redundant_path_length(path: &str) -> Option<usize> {
    let bytes = path.as_bytes();
    if bytes.first() != Some(&b'.') || bytes.get(1) != Some(&b'/') {
        return None;
    }
    let slashes = bytes[1..].iter().take_while(|byte| **byte == b'/').count();
    Some(1 + slashes)
}
