//! `Style/FileTouch`: opening a file for appending and writing nothing is `FileUtils.touch`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, is_string, string_text};

/// `APPEND_FILE_MODES`.
const APPEND_FILE_MODES: &[&str] = &["a", "a+", "ab", "a+b", "at", "a+t"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // The offense is reported on the `block` node, which is this call with its block here.
        let Some(block) = node.field("block") else {
            continue;
        };
        // `empty_block?`: a block that writes nothing is what `touch` stands for.
        if block.field("body").is_some() {
            continue;
        }
        let (Some(selector), Some(receiver)) = (node.field("method"), node.field("receiver"))
        else {
            continue;
        };
        if !is_plain_send(node, context) || context.source.node_text(selector) != "open" {
            continue;
        }
        if !super::nodes::is_top_level_constant(receiver, "File", context) {
            continue;
        }
        let list = arguments(node);
        let [filename, mode] = list.as_slice() else {
            continue;
        };
        if !is_append_mode(mode.first(), context) {
            continue;
        }
        let filename = context.source.slice(filename.range());
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `FileUtils.touch({filename})` instead of `File.open` in append mode \
                         with empty block."
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!("FileUtils.touch({filename})"),
                    safe: true,
                }),
        );
    }
}

/// `(str %APPEND_FILE_MODES)`.
fn is_append_mode(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    is_string(node, context) && APPEND_FILE_MODES.contains(&string_text(node, context))
}
