use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Every node kind whose condition upstream inspects: `on_if`, `on_while` and `on_until`, and the
/// modifier and post-loop spellings of the same three.
const CONDITIONALS: &[&str] = &[
    "if",
    "unless",
    "elsif",
    "while",
    "until",
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

/// The node kinds a conditional written in modifier form comes out as, which `modifier_op?` keeps
/// the parentheses around.
const MODIFIERS: &[&str] = &[
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_safe_assignment: bool = context.setting("AllowSafeAssignment").unwrap_or(true);
    let allow_multiline: bool = context
        .setting("AllowInMultilineConditions")
        .unwrap_or(false);
    for node in context.nodes_of_any(CONDITIONALS) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        // `(begin $_ $...)`: the condition has to be a parenthesized expression holding something.
        if condition.kind_str() != "parenthesized_statements" {
            continue;
        }
        let children = super::nodes::children_in(condition, context);
        let Some(first) = children.first().copied() else {
            continue;
        };
        if requires_parentheses(context, node, first)
            || semicolon_separated(context, &children)
            || MODIFIERS.contains(&first.kind_str())
            || first.kind_str() == "rescue_modifier"
            || parens_allowed(context, condition, allow_safe_assignment, allow_multiline)
        {
            continue;
        }
        let keyword = keyword(node);
        let article = match keyword {
            "while" => "a",
            _ => "an",
        };
        offenses.push(
            context
                .offense(
                    format!("Don't use parentheses around the condition of {article} `{keyword}`."),
                    condition.byte_range(),
                )
                .corrected_by_all(unwrap(context, condition)),
        );
    }
}

/// The keyword the conditional is written with, which the message names.
fn keyword(node: Node<'_>) -> &'static str {
    match node.kind_str() {
        "unless" | "unless_modifier" => "unless",
        "elsif" => "elsif",
        "while" | "while_modifier" => "while",
        "until" | "until_modifier" => "until",
        _ => "if",
    }
}

/// `require_parentheses?`: a `do ... end` block written as a loop's condition keeps them, or the
/// block would attach to the loop instead.
fn requires_parentheses(context: &RuleContext<'_>, node: Node<'_>, body: Node<'_>) -> bool {
    if !matches!(
        node.kind_str(),
        "while" | "until" | "while_modifier" | "until_modifier"
    ) {
        return false;
    }
    body.field("block")
        .is_some_and(|block| context.source.node_text(block).starts_with("do"))
}

/// `semicolon_separated_expressions?`: `(a; b)` is a sequence, not a parenthesized expression.
fn semicolon_separated(context: &RuleContext<'_>, children: &[Node<'_>]) -> bool {
    let [first, second, ..] = children else {
        return false;
    };
    context
        .source
        .slice(first.end_byte()..second.start_byte())
        .contains(';')
}

fn parens_allowed(
    context: &RuleContext<'_>,
    condition: Node<'_>,
    allow_safe_assignment: bool,
    allow_multiline: bool,
) -> bool {
    if parens_required(context, condition) {
        return true;
    }
    if allow_safe_assignment && is_safe_assignment(context, condition) {
        return true;
    }
    allow_multiline && {
        let range = condition.byte_range();
        context.source.line_column(range.start).0 != context.source.line_column(range.end).0
    }
}

/// `parens_required?`: a letter written directly against either parenthesis makes them part of a
/// call rather than a grouping.
fn parens_required(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let text = context.source.text().as_bytes();
    let before = node
        .start_byte()
        .checked_sub(1)
        .and_then(|index| text.get(index));
    let after = text.get(node.end_byte());
    [before, after]
        .into_iter()
        .flatten()
        .any(|byte| byte.is_ascii_lowercase())
}

/// `safe_assignment?`: `(begin {equals_asgn? #setter_method?})`, the parenthesized assignment that
/// says the assignment was meant.
fn is_safe_assignment(context: &RuleContext<'_>, condition: Node<'_>) -> bool {
    let children = super::nodes::children_in(condition, context);
    let [only] = children.as_slice() else {
        return false;
    };
    match only.kind_str() {
        "assignment" => !super::nodes::is_match_assignment(*only, context.source.text()),
        "call" => only
            .field("method")
            .is_some_and(|selector| context.source.node_text(selector).ends_with('=')),
        _ => false,
    }
}

/// `ParenthesesCorrector.correct`: the opening parenthesis takes the whitespace after it, and the
/// closing one the whitespace before it.
fn unwrap(context: &RuleContext<'_>, condition: Node<'_>) -> Vec<Edit> {
    let text = context.source.text();
    let range = condition.byte_range();
    let mut open_end = range.start + 1;
    while text
        .as_bytes()
        .get(open_end)
        .is_some_and(u8::is_ascii_whitespace)
    {
        open_end += 1;
    }
    let close_start = range.end - 1;
    // The newline before `)` is kept where a comment sits above it and a chain follows it, which
    // would otherwise pull the chain into the comment.
    let newlines = !comment_above_close_paren(context, condition);
    let before_close = super::ranges::extended_left(text, close_start, newlines);
    vec![
        Edit {
            start: range.start,
            end: open_end,
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: before_close,
            end: range.end,
            replacement: String::new(),
            safe: true,
        },
    ]
}

fn comment_above_close_paren(context: &RuleContext<'_>, condition: Node<'_>) -> bool {
    let Some(last) = super::nodes::children_in(condition, context).last().copied() else {
        return false;
    };
    let close = condition.end_byte() - 1;
    if last.end_byte() >= close {
        return false;
    }
    let between = context.source.slice(last.end_byte()..close);
    if !between
        .split_inclusive('\n')
        .any(|line| line.contains('#') && line.ends_with('\n'))
    {
        return false;
    }
    // `chained_after_close_paren?`: something other than a comment follows the `)` on its line.
    let line = context.source.line(context.source.line_column(close).0);
    let column = context.source.line_column(close).1;
    let after: String = line.chars().skip(column).collect();
    let trimmed = after.trim_start().trim_end_matches(['\n', '\r']);
    !trimmed.is_empty() && !trimmed.starts_with('#')
}
