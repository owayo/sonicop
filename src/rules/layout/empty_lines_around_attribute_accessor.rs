//! `Layout/EmptyLinesAroundAttributeAccessor`.

use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use super::support::statement_groups;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const ACCESSORS: [&str; 4] = ["attr_reader", "attr_writer", "attr_accessor", "attr"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_alias = context.setting::<bool>("AllowAliasSyntax").unwrap_or(true);
    let allowed: Vec<String> = context
        .setting::<Vec<String>>("AllowedMethods")
        .unwrap_or_default();
    // `node.right_sibling`: what follows the accessor in the statement list it belongs to. A
    // definition that is a body of its own has no sibling, and neither has one written as a branch
    // of an `if`, which upstream skips outright.
    let mut siblings: HashMap<usize, Node<'_>> = HashMap::new();
    for group in statement_groups(context) {
        for pair in group.statements.windows(2) {
            siblings.insert(pair[0].id(), pair[1]);
        }
    }

    for node in context.nodes_of("call") {
        if !is_attribute_accessor(context, node) {
            continue;
        }
        let last_line = context.source.line_column(node.end_byte()).0;
        if next_line_empty_or_enable_directive(context, last_line) {
            continue;
        }
        let Some(sibling) = siblings.get(&node.id()) else {
            continue;
        };
        if !requires_empty_line(context, *sibling, allow_alias, &allowed) {
            continue;
        }
        let anchor = correction_anchor(context, node, last_line);
        offenses.push(
            context
                .offense(
                    "Add an empty line after attribute accessor.",
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: anchor.end,
                    end: anchor.end,
                    replacement: "\n".to_owned(),
                    safe: true,
                })
                .corrections_anchored_at(anchor),
        );
    }
}

/// `autocorrect`: the newline lands after the accessor's last line, or after the `rubocop:enable`
/// comment that follows it, so that the directive keeps covering the accessor.
fn correction_anchor(context: &RuleContext<'_>, node: Node<'_>, last_line: usize) -> Range<usize> {
    if let Some(comment) = enable_directive_at(context, last_line + 1) {
        return comment;
    }
    let first_line = context.source.line_column(node.start_byte()).0;
    whole_lines(context, first_line, last_line)
}

/// `range_by_whole_lines`: the lines the node sits on, without the final newline.
fn whole_lines(context: &RuleContext<'_>, first_line: usize, last_line: usize) -> Range<usize> {
    let start = context.source.line_start(first_line);
    let line = context.source.line_range(last_line);
    let end = match context.source.text()[line.clone()].ends_with('\n') {
        true => line.end - 1,
        false => line.end,
    };
    start..end.max(start)
}

/// `next_line_empty_or_enable_directive_comment?`.
fn next_line_empty_or_enable_directive(context: &RuleContext<'_>, line: usize) -> bool {
    if is_blank_line(context, line + 1) {
        return true;
    }
    enable_directive_at(context, line + 1).is_some() && is_blank_line(context, line + 2)
}

/// `next_line_empty?`: the line is blank, or past the end of the file.
fn is_blank_line(context: &RuleContext<'_>, line: usize) -> bool {
    context.source.line(line).trim().is_empty()
}

fn is_attribute_accessor(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.field("receiver").is_some() {
        return false;
    }
    let Some(method) = node.field("method") else {
        return false;
    };
    if !ACCESSORS.contains(&&context.source.text()[method.byte_range()]) {
        return false;
    }
    // `(_ _ _ _ ...)`: the accessor has to name at least one attribute.
    node.field("arguments")
        .is_some_and(|arguments| arguments.named_child_count() > 0)
}

/// `require_empty_line?`: an accessor followed by another accessor, an alias or an allowed method
/// is still one group.
fn requires_empty_line(
    context: &RuleContext<'_>,
    node: Node<'_>,
    allow_alias: bool,
    allowed: &[String],
) -> bool {
    if allow_alias && node.kind_str() == "alias" {
        return false;
    }
    if is_attribute_accessor(context, node) {
        return false;
    }
    let name = match node.kind_str() {
        "identifier" => &context.source.text()[node.byte_range()],
        "call" | "method_call" => match node.field("method") {
            Some(method) => &context.source.text()[method.byte_range()],
            None => return true,
        },
        _ => return true,
    };
    !allowed.iter().any(|method| method == name)
}

/// The `rubocop:enable` comment written on `line`, if there is one.
fn enable_directive_at(context: &RuleContext<'_>, line: usize) -> Option<Range<usize>> {
    let comment = context
        .comment_ranges()
        .iter()
        .find(|range| context.source.line_column(range.start).0 == line)?;
    let text = &context.source.text()[comment.clone()];
    is_enable_directive(text).then(|| comment.clone())
}

/// `DirectiveComment#enabled?`: the comment's first `# rubocop:<mode>` header names `enable`.
/// A header that only a second `#` precedes is prose, and disables the whole comment as a
/// directive rather than being skipped over.
fn is_enable_directive(text: &str) -> bool {
    for (index, _) in text.match_indices('#') {
        let Some(mode) = directive_mode(&text[index..]) else {
            continue;
        };
        let prefix = &text[..index];
        if prefix.starts_with('#') && prefix[1..].chars().all(char::is_whitespace) {
            return false;
        }
        return mode == "enable";
    }
    false
}

/// `DIRECTIVE_HEADER_PATTERN` anchored at `text`, which starts at a `#`.
fn directive_mode(text: &str) -> Option<&str> {
    let rest = text[1..].trim_start();
    let rest = rest.strip_prefix("rubocop")?.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    ["disable", "enable", "todo", "push", "pop"]
        .into_iter()
        .find(|mode| {
            rest.strip_prefix(mode).is_some_and(|after| {
                !after
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_alphanumeric() || character == '_')
            })
        })
}
