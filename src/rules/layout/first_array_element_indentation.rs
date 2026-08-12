//! `Layout/FirstArrayElementIndentation`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    IndentBase, alignment_corrections, argument_literals, character_column, holds_block_comment,
    indent_base, literal_opening, preceded_by_code, string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

/// `%w[...]` and `%i[...]` are arrays upstream, so their opener counts as a left bracket.
const ARRAY_KINDS: [&str; 3] = ["array", "string_array", "symbol_array"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "special_inside_parentheses".to_owned());
    // Only the `consistent` style survives the neighbouring cop's fixed indentation.
    if style != "consistent"
        && context
            .setting_of::<String>("Layout/ArrayAlignment", "EnforcedStyle")
            .as_deref()
            == Some("with_fixed_indentation")
    {
        return;
    }
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);

    let mut claimed: HashSet<usize> = HashSet::new();
    for call in context.nodes_of("call") {
        for (array, parenthesis) in argument_literals(context, call, &ARRAY_KINDS) {
            if claimed.insert(array.id()) {
                inspect(context, &style, width, array, Some(parenthesis), offenses);
            }
        }
    }
    for array in context.nodes_of_any(&ARRAY_KINDS) {
        if !claimed.contains(&array.id()) {
            inspect(context, &style, width, array, None, offenses);
        }
    }
}

fn inspect(
    context: &RuleContext<'_>,
    style: &str,
    width: i64,
    array: Node<'_>,
    parenthesis: Option<Node<'_>>,
    offenses: &mut Vec<Offense>,
) {
    let Some(open) = literal_opening(array) else {
        return;
    };
    let first = first_element(array);
    if let Some((span, node)) = &first {
        if context.source.line_column(span.start).0 == open.start_position().row + 1 {
            return;
        }
        check_first(
            context,
            style,
            width,
            open,
            span.clone(),
            *node,
            parenthesis,
            offenses,
        );
    }
    let first_node = first.as_ref().and_then(|(_, node)| *node);
    check_right_bracket(context, style, array, first_node, open, parenthesis, offenses);
}

/// `array_node.values.first`. A run of `key: value` pairs is one `hash` value upstream, so the
/// first element spans the whole run rather than just its first pair.
fn first_element<'tree>(array: Node<'tree>) -> Option<(Range<usize>, Option<Node<'tree>>)> {
    let mut cursor = array.walk();
    let children: Vec<Node<'tree>> = array
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
        .collect();
    let first = *children.first()?;
    if !matches!(first.kind(), "pair" | "hash_splat_argument") {
        return Some((first.byte_range(), Some(first)));
    }
    let end = children
        .iter()
        .take_while(|child| matches!(child.kind(), "pair" | "hash_splat_argument"))
        .last()?
        .end_byte();
    Some((first.start_byte()..end, None))
}

#[allow(clippy::too_many_arguments)]
fn check_first(
    context: &RuleContext<'_>,
    style: &str,
    width: i64,
    open: Node<'_>,
    span: Range<usize>,
    first: Option<Node<'_>>,
    parenthesis: Option<Node<'_>>,
    offenses: &mut Vec<Offense>,
) {
    let actual = character_column(context, span.start);
    let (base, kind) = indent_base(context, open, first, parenthesis, style, "align_brackets");
    let delta = base + width - actual;
    if delta == 0 {
        return;
    }
    let message = format!(
        "Use {width} spaces for indentation in an array, relative to {}.",
        base_description(kind)
    );
    let mut offense = context.offense(message, span.clone());
    if !holds_block_comment(context, &span) {
        let taboo = string_interiors(context, &span);
        offense = offense.corrected_by_all(alignment_corrections(context, span, delta, &taboo));
    }
    offenses.push(offense);
}

fn check_right_bracket(
    context: &RuleContext<'_>,
    style: &str,
    array: Node<'_>,
    first: Option<Node<'_>>,
    open: Node<'_>,
    parenthesis: Option<Node<'_>>,
    offenses: &mut Vec<Offense>,
) {
    let Some(close) = closing(array) else { return };
    if preceded_by_code(context, close.start_byte()) {
        return;
    }
    let (base, kind) = indent_base(context, open, first, parenthesis, style, "align_brackets");
    let delta = base - character_column(context, close.start_byte());
    if delta == 0 {
        return;
    }
    let span = close.byte_range();
    let taboo = string_interiors(context, &span);
    offenses.push(
        context
            .offense(right_bracket_message(kind), span.clone())
            .corrected_by_all(alignment_corrections(context, span, delta, &taboo)),
    );
}

fn closing<'tree>(array: Node<'tree>) -> Option<Node<'tree>> {
    let count = array.child_count();
    array
        .child(u32::try_from(count).ok()?.checked_sub(1)?)
        .filter(|child| matches!(child.kind(), "]" | ")"))
}

fn base_description(kind: IndentBase) -> &'static str {
    match kind {
        IndentBase::LeftBraceOrBracket => "the position of the opening bracket",
        IndentBase::FirstColumnAfterLeftParenthesis => {
            "the first position after the preceding left parenthesis"
        }
        IndentBase::ParentHashKey => "the parent hash key",
        IndentBase::StartOfLine => "the start of the line where the left square bracket is",
    }
}

fn right_bracket_message(kind: IndentBase) -> &'static str {
    match kind {
        IndentBase::LeftBraceOrBracket => "Indent the right bracket the same as the left bracket.",
        IndentBase::FirstColumnAfterLeftParenthesis => {
            "Indent the right bracket the same as the first position after the preceding left \
             parenthesis."
        }
        IndentBase::ParentHashKey => "Indent the right bracket the same as the parent hash key.",
        IndentBase::StartOfLine => {
            "Indent the right bracket the same as the start of the line where the left bracket is."
        }
    }
}
