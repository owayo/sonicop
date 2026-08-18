//! `Style/DocumentDynamicEvalDefinition`: what an interpolated `eval` string defines is unreadable
//! until a comment spells it out.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{is_plain_send, arguments};

const MSG: &str = "Add a comment block showing its appearance if interpolated.";

/// `RESTRICT_ON_SEND`.
const EVAL_METHODS: &[&str] = &["eval", "class_eval", "module_eval", "instance_eval"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        let Some(selector) = node.field("method") else {
            continue;
        };
        if !EVAL_METHODS.contains(&context.source.node_text(selector)) {
            continue;
        }
        let list = arguments(node);
        let Some(argument) = list.first() else {
            continue;
        };
        let argument = argument.first();
        let heredoc = argument.kind_str() == "heredoc_beginning";
        // `arg_node.dstr_type? && interpolated?`: the parts of the literal, and at least one of
        // them has to be an interpolation.
        let Some(parts) = literal_parts(argument, context) else {
            continue;
        };
        if !parts.iter().any(|part| part.kind_str() == "interpolation") {
            continue;
        }
        // `inline_comment_docs?`: every interpolation's own line carries a comment.
        if parts
            .iter()
            .filter(|part| part.kind_str() == "interpolation")
            .all(|part| {
                let (line, _) = context.source.line_column(part.start_byte());
                has_comment(context.source.line(line))
            })
        {
            continue;
        }
        if heredoc && comment_block_docs(context, node, argument, &parts) {
            continue;
        }
        offenses.push(context.offense(MSG, selector.byte_range()));
    }
}

/// The `str` and `begin` children upstream's `dstr` holds.
///
/// A heredoc keeps them in the body the grammar parks after the statement, and the terminator is
/// no part of the literal.
fn literal_parts<'tree>(
    argument: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Vec<Node<'tree>>> {
    let container = match argument.kind_str() {
        "heredoc_beginning" => crate::rules::send_node::heredoc_body(argument, context)?,
        "string" | "chained_string" => argument,
        _ => return None,
    };
    Some(
        super::nodes::children(container)
            .into_iter()
            .filter(|child| child.kind_str() != "heredoc_end")
            .collect(),
    )
}

/// `comment_block_docs?`: a comment block that reads like what the interpolation produces.
fn comment_block_docs(
    context: &RuleContext<'_>,
    send: Node<'_>,
    argument: Node<'_>,
    parts: &[Node<'_>],
) -> bool {
    let Some(body) = crate::rules::send_node::heredoc_body(argument, context) else {
        return false;
    };
    let mut comments = heredoc_comment_blocks(context, body);
    comments.extend(preceding_comment_blocks(context, send));
    if comments.is_empty() {
        return false;
    }
    let Some(pattern) = comment_regexp(context, parts) else {
        return false;
    };
    let Ok(pattern) = regex::Regex::new(&pattern) else {
        return false;
    };
    comments.iter().any(|comment| pattern.is_match(comment)) || pattern.is_match(&comments.concat())
}

/// `heredoc_comment_blocks`: the comment lines written inside the heredoc, adjacent ones joined.
fn heredoc_comment_blocks(context: &RuleContext<'_>, body: Node<'_>) -> Vec<String> {
    let (first, _) = context.source.line_column(body.start_byte());
    let (last, _) = context.source.line_column(body.end_byte());
    let lines = (first + 1..=last).map(|line| (line, context.source.line(line).to_owned()));
    merge_adjacent(lines)
}

/// `preceding_comment_blocks`: the comments written on the call's own lines.
fn preceding_comment_blocks(context: &RuleContext<'_>, send: Node<'_>) -> Vec<String> {
    let (first, _) = context.source.line_column(send.start_byte());
    let (last, _) = context.source.line_column(send.end_byte());
    let found = context
        .comment_ranges()
        .iter()
        .map(|comment| {
            let (line, _) = context.source.line_column(comment.start);
            (line, context.source.slice(comment.clone()).to_owned())
        })
        .filter(|(line, _)| (first..=last).contains(line));
    merge_adjacent(found)
}

/// `merge_adjacent_comments`: a line that is a comment loses its `#`, and a run of them becomes one
/// block.
fn merge_adjacent(lines: impl Iterator<Item = (usize, String)>) -> Vec<String> {
    let mut blocks: Vec<(usize, String)> = Vec::new();
    for (index, line) in lines {
        let Some(stripped) = strip_comment_marker(&line) else {
            continue;
        };
        match blocks.last_mut() {
            Some((last, block)) if *last + 1 == index => {
                block.push('\n');
                block.push_str(&stripped);
                *last = index;
            }
            _ => blocks.push((index, stripped)),
        }
    }
    blocks.into_iter().map(|(_, block)| block).collect()
}

/// `BLOCK_COMMENT_REGEXP`: `/^\s*#(?!{)/`, which only bites at the start of the line.
fn strip_comment_marker(line: &str) -> Option<String> {
    let trimmed = crate::rules::support::chomp(line);
    let rest = trimmed.trim_start();
    let marker = rest.strip_prefix('#')?;
    if marker.starts_with('{') {
        return None;
    }
    Some(marker.to_owned())
}

/// `comment_regexp`: the literal read as a pattern, with each interpolation standing for anything.
fn comment_regexp(context: &RuleContext<'_>, parts: &[Node<'_>]) -> Option<String> {
    let mut pattern = String::new();
    for (index, part) in parts.iter().enumerate() {
        if part.kind_str() == "interpolation" {
            pattern.push_str(".+");
            continue;
        }
        let mut source = context.source.node_text(*part);
        // The grammar's first content run starts where the opener was written; upstream's begins on
        // the line below it.
        if index == 0
            && let Some((_, rest)) = source.split_once('\n')
        {
            source = rest;
        }
        pattern.push_str(&source_to_regexp(source));
    }
    (!pattern.is_empty()).then_some(pattern)
}

/// `source_to_regexp`.
fn source_to_regexp(source: &str) -> String {
    if is_blank(source) {
        return "\\s*".to_owned();
    }
    let source = remove_comments(source);
    if is_blank(&source) {
        return String::new();
    }
    let escaped = source
        .trim()
        .split("\\#")
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join("\\\\?#");
    format!("\\s*{escaped}")
}

/// `String#blank?`.
fn is_blank(value: &str) -> bool {
    value.chars().all(char::is_whitespace)
}

/// `source.gsub(COMMENT_REGEXP, '')` for `/\s*#(?!{).*/`.
///
/// The `\s*` in front reaches back over the blanks before the `#`, line breaks included, and the
/// `.*` behind it stops at the next one. Rust's regex engine has no lookahead, so the scan is
/// written out.
fn remove_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut kept = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'#' || bytes.get(index + 1) == Some(&b'{') {
            index += 1;
            continue;
        }
        let mut start = index;
        while start > kept && bytes[start - 1].is_ascii_whitespace() {
            start -= 1;
        }
        let end = source[index..]
            .find('\n')
            .map_or(source.len(), |offset| index + offset);
        out.push_str(&source[kept..start]);
        kept = end;
        index = end;
    }
    out.push_str(&source[kept..]);
    out
}

/// `COMMENT_REGEXP` used as a test on one line: a `#` that does not open an interpolation.
fn has_comment(line: &str) -> bool {
    let bytes = line.as_bytes();
    (0..bytes.len()).any(|index| bytes[index] == b'#' && bytes.get(index + 1) != Some(&b'{'))
}
