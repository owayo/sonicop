//! `Layout/FirstHashElementIndentation`.

use std::collections::HashSet;

use tree_sitter::Node;

use super::support::{
    IndentBase, alignment_corrections, argument_literals, character_column, holds_block_comment,
    indent_base, literal_opening, preceded_by_code, string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "special_inside_parentheses".to_owned());
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    // A hash the neighbouring cop indents by a fixed amount is left to that cop entirely.
    let fixed_arguments = context
        .setting_of::<String>("Layout/ArgumentAlignment", "EnforcedStyle")
        .as_deref()
        == Some("with_fixed_indentation");

    let mut claimed: HashSet<usize> = HashSet::new();
    if !fixed_arguments {
        for call in context.nodes_of("call") {
            for (hash, parenthesis) in argument_literals(context, call, &["hash"]) {
                if claimed.insert(hash.id()) {
                    inspect(context, &style, width, hash, Some(parenthesis), offenses);
                }
            }
        }
    }
    for hash in context.nodes_of("hash") {
        if !claimed.contains(&hash.id()) {
            inspect(context, &style, width, hash, None, offenses);
        }
    }
}

fn inspect(
    context: &RuleContext<'_>,
    style: &str,
    width: i64,
    hash: Node<'_>,
    parenthesis: Option<Node<'_>>,
    offenses: &mut Vec<Offense>,
) {
    let Some(open) = literal_opening(hash) else {
        return;
    };
    let first = first_pair(hash);
    if let Some(first) = first {
        if first.start_position().row == open.start_position().row {
            return;
        }
        // `check_based_on_longest_key` only applies where the neighbouring cop lines pairs up on
        // their separators, which shifts the first key right by the widest key's overhang.
        let offset = if separator_style(context, first) {
            longest_key_overhang(context, hash)
        } else {
            0
        };
        check_first(
            context,
            style,
            width,
            open,
            first,
            parenthesis,
            offset,
            offenses,
        );
    }
    check_right_brace(context, style, hash, first, open, parenthesis, offenses);
}

fn first_pair<'tree>(hash: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = hash.walk();
    hash.named_children(&mut cursor)
        .find(|child| child.kind_str() == "pair")
}

fn separator_style(context: &RuleContext<'_>, first: Node<'_>) -> bool {
    let mut cursor = first.walk();
    let rocket = first
        .children(&mut cursor)
        .any(|child| child.kind_str() == "=>");
    let key = if rocket {
        "EnforcedHashRocketStyle"
    } else {
        "EnforcedColonStyle"
    };
    context
        .setting_of::<String>("Layout/HashAlignment", key)
        .as_deref()
        == Some("separator")
}

fn longest_key_overhang(context: &RuleContext<'_>, hash: Node<'_>) -> i64 {
    let mut cursor = hash.walk();
    let lengths: Vec<i64> = hash
        .named_children(&mut cursor)
        .filter(|child| child.kind_str() == "pair")
        .filter_map(|pair| pair.field("key"))
        .map(|key| context.source.text()[key.byte_range()].chars().count() as i64)
        .collect();
    let Some(first) = lengths.first() else {
        return 0;
    };
    lengths.iter().copied().max().unwrap_or(0) - first
}

#[allow(clippy::too_many_arguments)]
fn check_first(
    context: &RuleContext<'_>,
    style: &str,
    width: i64,
    open: Node<'_>,
    first: Node<'_>,
    parenthesis: Option<Node<'_>>,
    offset: i64,
    offenses: &mut Vec<Offense>,
) {
    let actual = character_column(context, first.start_byte());
    let (base, kind) = indent_base(
        context,
        open,
        Some(first),
        parenthesis,
        style,
        "align_braces",
    );
    let delta = base + width + offset - actual;
    if delta == 0 {
        return;
    }
    let message = format!(
        "Use {width} spaces for indentation in a hash, relative to {}.",
        base_description(kind)
    );
    // A pair whose value opens on a later line is moved by its first line alone, so that the
    // value's own lines keep the indentation they were given.
    let span = match (
        first.field("key"),
        first.field("value"),
    ) {
        (Some(key), Some(value)) if value.start_position().row > key.start_position().row => {
            line_span(context, first.start_byte())
        }
        _ => first.byte_range(),
    };
    let mut offense = context.offense(message, first.byte_range());
    if !holds_block_comment(context, &span) {
        let taboo = string_interiors(context, &span);
        offense = offense.corrected_by_all(alignment_corrections(context, span, delta, &taboo));
    }
    offenses.push(offense);
}

#[allow(clippy::too_many_arguments)]
fn check_right_brace(
    context: &RuleContext<'_>,
    style: &str,
    hash: Node<'_>,
    first: Option<Node<'_>>,
    open: Node<'_>,
    parenthesis: Option<Node<'_>>,
    offenses: &mut Vec<Offense>,
) {
    let Some(close) = closing(hash) else { return };
    if preceded_by_code(context, close.start_byte()) {
        return;
    }
    let (base, kind) = indent_base(context, open, first, parenthesis, style, "align_braces");
    let delta = base - character_column(context, close.start_byte());
    if delta == 0 {
        return;
    }
    let span = close.byte_range();
    let taboo = string_interiors(context, &span);
    offenses.push(
        context
            .offense(right_brace_message(kind), span.clone())
            .corrected_by_all(alignment_corrections(context, span, delta, &taboo)),
    );
}

fn closing<'tree>(hash: Node<'tree>) -> Option<Node<'tree>> {
    let count = hash.child_count();
    hash.child(u32::try_from(count).ok()?.checked_sub(1)?)
        .filter(|child| child.kind_str() == "}")
}

fn line_span(context: &RuleContext<'_>, offset: usize) -> std::ops::Range<usize> {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    start..start + context.source.line(line).trim_end_matches('\n').len()
}

fn base_description(kind: IndentBase) -> &'static str {
    match kind {
        IndentBase::LeftBraceOrBracket => "the position of the opening brace",
        IndentBase::FirstColumnAfterLeftParenthesis => {
            "the first position after the preceding left parenthesis"
        }
        IndentBase::ParentHashKey => "the parent hash key",
        IndentBase::StartOfLine => "the start of the line where the left curly brace is",
    }
}

fn right_brace_message(kind: IndentBase) -> &'static str {
    match kind {
        IndentBase::LeftBraceOrBracket => "Indent the right brace the same as the left brace.",
        IndentBase::FirstColumnAfterLeftParenthesis => {
            "Indent the right brace the same as the first position after the preceding left \
             parenthesis."
        }
        IndentBase::ParentHashKey => "Indent the right brace the same as the parent hash key.",
        IndentBase::StartOfLine => {
            "Indent the right brace the same as the start of the line where the left brace is."
        }
    }
}
