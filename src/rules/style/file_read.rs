//! `Style/FileRead`: opening a file only to read it whole is `File.read`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

/// `READ_FILE_START_TO_FINISH_MODES`.
const READ_MODES: &[&str] = &["r", "rt", "rb", "r+", "r+t", "r+b"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(opened) = super::file_open_call::parse(node, context, READ_MODES, "read") else {
            continue;
        };
        // `read_node?`: the read is either handed in as `&:read`, written inside the block the
        // file was opened with, or written on the result.
        //
        // The first two are the call itself here. A block makes the call a `block` node upstream,
        // which is what `node.parent` is then -- so a block that does something else is the end of
        // it, and what encloses the call is never looked at.
        let read_node = if opened.block_pass {
            node.byte_range()
        } else if node.field("block").is_some() {
            if super::file_open_call::block_calls(node, context, "read", 0).is_none() {
                continue;
            }
            node.byte_range()
        } else {
            match context.parent(node) {
                Some(parent)
                    if parent.kind_str() == "call"
                        && parent
                            .field("method")
                            .is_some_and(|name| context.source.node_text(name) == "read")
                        && arguments(parent).is_empty()
                        && is_plain_send(parent, context)
                        && parent.field("block").is_none() =>
                {
                    parent.byte_range()
                }
                _ => continue,
            }
        };
        let method = read_method(opened.mode.unwrap_or("r"));
        let Some(selector) = node.field("method") else {
            continue;
        };
        let range = selector.start_byte()..read_node.end;
        offenses.push(
            context
                .offense(format!("Use `File.{method}`."), read_node)
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: format!("{method}({})", context.source.slice(opened.filename)),
                    safe: true,
                }),
        );
    }
}

/// `read_method`: a mode ending in `b` reads bytes.
fn read_method(mode: &str) -> &'static str {
    if mode.ends_with('b') {
        "binread"
    } else {
        "read"
    }
}
