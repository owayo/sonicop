//! `Style/FileWrite`: opening a file only to write it whole is `File.write`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

/// `TRUNCATING_WRITE_MODES`.
const WRITE_MODES: &[&str] = &["w", "wt", "wb", "w+", "w+t", "w+b"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(opened) = super::file_open_call::parse(node, context, WRITE_MODES, "write") else {
            continue;
        };
        // Unlike the read pattern, the mode is not optional: only a truncating open writes the
        // file whole.
        let Some(mode) = opened.mode else {
            continue;
        };
        // A block makes the call a `block` node upstream, which is what `node.parent` is then, so
        // a block that writes something else is the end of it.
        let (content, write_node) = if node.field("block").is_some() {
            match super::file_open_call::block_calls(node, context, "write", 1) {
                Some(written) => (written[0].clone(), node.byte_range()),
                None => continue,
            }
        } else {
            match written_by_a_further_call(node, context) {
                Some(content) => (content, context.parent(node).unwrap_or(node).byte_range()),
                None => continue,
            }
        };
        // `return false if content&.splat_type?`: a splat is several values, not one payload.
        if context.source.slice(content.clone()).starts_with('*') {
            continue;
        }
        let method = if mode.ends_with('b') {
            "binwrite"
        } else {
            "write"
        };
        let Some(selector) = node.field("method") else {
            continue;
        };
        let range = selector.start_byte()..write_node.end;
        // A heredoc opened inside the range is written below the line the range sits on, so the
        // replacement has to carry its body along or the correction would delete it.
        let mut replacement = format!(
            "{method}({}, {})",
            context.source.slice(opened.filename.clone()),
            context.source.slice(content.clone())
        );
        for body in removed_heredocs(context, &[opened.filename, content], write_node.end) {
            replacement.push('\n');
            replacement.push_str(context.source.slice(body));
        }
        offenses.push(
            context
                .offense(format!("Use `File.{method}`."), write_node)
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `send_write?`: `File.open(path, 'w').write(content)`, answering with the content.
fn written_by_a_further_call(node: Node<'_>, context: &RuleContext<'_>) -> Option<Range<usize>> {
    let parent = context.parent(node)?;
    if parent.kind_str() != "call" || !is_plain_send(parent, context) {
        return None;
    }
    if context.source.node_text(parent.field("method")?) != "write" {
        return None;
    }
    let list = arguments(parent);
    let [content] = list.as_slice() else {
        return None;
    };
    Some(content.range())
}

/// `removed_heredocs`: the bodies a correction would swallow, in the order they were written.
///
/// A heredoc's body is parked below the line that opened it, so a replacement reaching past the
/// terminator takes the body with it. Upstream's `heredoc_body` starts on the line below the
/// opener, which is one newline past where the grammar's `heredoc_content` begins.
fn removed_heredocs(
    context: &RuleContext<'_>,
    within: &[Range<usize>],
    end: usize,
) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = Vec::new();
    for opener in context.nodes_of("heredoc_beginning") {
        if !within
            .iter()
            .any(|range| range.start <= opener.start_byte() && opener.end_byte() <= range.end)
        {
            continue;
        }
        let Some(body) = crate::rules::send_node::heredoc_body(opener, context) else {
            continue;
        };
        let parts = super::nodes::children(body);
        let Some(terminator) = parts.iter().find(|part| part.kind_str() == "heredoc_end") else {
            continue;
        };
        if terminator.end_byte() > end {
            continue;
        }
        let start = match parts
            .iter()
            .find(|part| part.kind_str() == "heredoc_content")
        {
            Some(content) => context.source.text()[content.start_byte()..]
                .find('\n')
                .map_or(terminator.start_byte(), |offset| {
                    content.start_byte() + offset + 1
                }),
            None => terminator.start_byte(),
        };
        found.push(start..terminator.end_byte());
    }
    found.sort_by_key(|range| range.start);
    found
}
