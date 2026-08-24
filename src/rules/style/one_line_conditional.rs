use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The node kinds upstream's parser all builds an `if` node for, which is what decides whether a
/// branch needs parentheses around it.
const IF_KINDS: &[&str] = &[
    "if",
    "unless",
    "elsif",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let always_multiline: bool = context.setting("AlwaysCorrectToMultiline").unwrap_or(false);
    let width: usize = context
        .setting_of("Layout/IndentationWidth", "Width")
        .unwrap_or(2);
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["if", "unless"]) {
        let range = node.byte_range();
        if context.source.line_column(range.start).0 != context.source.line_column(range.end).0 {
            continue;
        }
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let Some(alternative) = node.field("alternative") else {
            continue;
        };
        // `return unless node.else_branch`: upstream's `else_branch` is what the clause **holds**,
        // so a bare `else end` answers `nil`. The grammar keeps the empty clause as a node, and
        // taking its presence for a branch made `if cond then run else end` look convertible.
        if !crate::rules::send_node::named_children(alternative)
            .iter()
            .any(|child| child.kind_str() != "comment")
        {
            continue;
        }
        let consequence = node.field("consequence");
        // `node.if_branch&.begin_type?`: a then-branch upstream reads as a `begin` has no ternary
        // arm to become.
        if consequence.is_some_and(|clause| begin_children(clause).is_some()) {
            continue;
        }
        let multiline = always_multiline || cannot_become_a_ternary(alternative);
        let keyword = node.kind_str();
        let message = match multiline {
            true => format!(
                "Favor multi-line `{keyword}` over single-line `{keyword}/then/else/end` constructs."
            ),
            false => format!(
                "Favor the ternary operator (`?:`) over single-line `{keyword}/then/else/end` constructs."
            ),
        };
        let mut offense = context.offense(message, range.clone());
        if !ignored
            .iter()
            .any(|ignored| ignored.start <= range.start && range.end <= ignored.end)
        {
            let replacement = match multiline {
                true => expanded(
                    context,
                    node,
                    width,
                    context.source.line_column(range.start).1 - 1,
                ),
                false => ternary(context, node, condition, consequence, alternative),
            };
            offense = offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement,
                safe: true,
            });
            ignored.push(range);
        }
        offenses.push(offense);
    }
}

/// `cannot_replace_to_ternary?`: an `elsif` chain, or an else branch holding more than one
/// statement.
fn cannot_become_a_ternary(alternative: Node<'_>) -> bool {
    alternative.kind_str() == "elsif"
        || begin_children(alternative).is_some_and(|count| count >= 2)
}

/// How many statements a clause holds when upstream reads it as a `begin`, which is either a run of
/// statements or one parenthesized expression.
fn begin_children(clause: Node<'_>) -> Option<usize> {
    let written = super::nodes::children(clause);
    match written.as_slice() {
        [only] if only.kind_str() == "parenthesized_statements" => {
            Some(super::nodes::children(*only).len())
        }
        _ if written.len() > 1 => Some(written.len()),
        _ => None,
    }
}

/// `ternary_replacement`, wrapped where the expression around it binds tighter.
fn ternary(
    context: &RuleContext<'_>,
    node: Node<'_>,
    condition: Node<'_>,
    consequence: Option<Node<'_>>,
    alternative: Node<'_>,
) -> String {
    // The parser writes an `unless` with its branches the other way round, and the ternary is
    // built from the children as they are stored rather than from the normalized branches.
    let (first, second) = match node.kind_str() {
        "unless" => (Some(alternative), consequence),
        _ => (consequence, Some(alternative)),
    };
    let replaced = format!(
        "{} ? {} : {}",
        branch(context, Some(condition)),
        branch(context, first),
        branch(context, second)
    );
    // An operator written around the conditional binds tighter than the ternary does.
    let wrapped = node.parent_of(context).is_some_and(|parent| match parent.kind_str() {
        "binary" | "element_reference" => true,
        // `defined?` is a node of its own upstream rather than an operator call.
        "unary" => parent
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) != "defined?"),
        "call" => parent
            .field("method")
            .is_some_and(|selector| selector.kind_str() == "operator"),
        _ => false,
    });
    match wrapped {
        true => format!("({replaced})"),
        false => replaced,
    }
}

/// `expr_replacement`: one arm of the ternary, parenthesized where it would otherwise be read as
/// part of the ternary.
fn branch(context: &RuleContext<'_>, node: Option<Node<'_>>) -> String {
    let Some(node) = node else {
        return "nil".to_owned();
    };
    let (source, node) = match node.kind_str() {
        "then" | "else" => match super::nodes::children(node).as_slice() {
            [only] => (context.source.node_text(*only).to_owned(), Some(*only)),
            [] => return "nil".to_owned(),
            [first, .., last] => (
                context
                    .source
                    .slice(first.start_byte()..last.end_byte())
                    .to_owned(),
                None,
            ),
        },
        _ => (context.source.node_text(node).to_owned(), Some(node)),
    };
    match node.is_some_and(|node| requires_parentheses(context, node)) {
        true => format!("({source})"),
        false => source,
    }
}

fn requires_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if IF_KINDS.contains(&node.kind_str()) {
        return true;
    }
    if matches!(node.kind_str(), "assignment" | "operator_assignment") {
        return true;
    }
    // `%i[and or if]` are **node types**, and upstream's parser builds an `or` for `||` just as it
    // does for `or`. Reading the two keyword spellings only leaves `a || b ? x : y`, where the
    // ternary binds tighter than the reader expects.
    if node.kind_str() == "binary"
        && node.field("operator").is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "and" | "or" | "&&" | "||"
            )
        })
    {
        return true;
    }
    // `method_call_with_changed_precedence?`: a call written without parentheses takes the `:` for
    // an argument.
    if node.kind_str() == "call"
        && let Some(arguments) = node.field("arguments")
        && !context.source.node_text(arguments).starts_with('(')
        && node
            .field("method")
            .is_some_and(|selector| selector.kind_str() != "operator")
    {
        return true;
    }
    // `keyword_with_changed_precedence?`: `not x`, and a keyword written with arguments -- which
    // includes `defined? :A`, a `defined?` node upstream and a `unary` here. Only `not` was read,
    // so `defined? :A` lost the parentheses that keep the `?` of the ternary out of its argument.
    if node.kind_str() == "unary" {
        return node.field("operator").is_some_and(|operator| {
            match context.source.node_text(operator) {
                "not" => true,
                "defined?" => node
                    .field("operand")
                    .is_some_and(|operand| !context.source.node_text(operand).starts_with('(')),
                _ => false,
            }
        });
    }
    // `node.arguments? && !node.parenthesized_call?`: a keyword whose arguments are already in
    // parentheses takes nothing more from what follows, so it needs no parentheses of its own.
    // `yield(2)` was coming out as `(yield(2))`.
    matches!(node.kind_str(), "return" | "break" | "next" | "yield")
        && node
            .child(1)
            .is_some_and(|arguments| !context.source.node_text(arguments).starts_with('('))
        && !super::nodes::children(node).is_empty()
}

/// `IfThenCorrector`: the same conditional written over several lines.
fn expanded(context: &RuleContext<'_>, node: Node<'_>, width: usize, column: usize) -> String {
    let indentation = " ".repeat(column);
    let body = " ".repeat(width);
    let keyword = match node.kind_str() {
        "unless" => "unless",
        "elsif" => "elsif",
        _ => "if",
    };
    let condition = node
        .field("condition")
        .map_or_else(String::new, |node| {
            context.source.node_text(node).to_owned()
        });
    let consequence = node
        .field("consequence")
        .map_or_else(|| "nil".to_owned(), |clause| clause_source(context, clause));
    // An `elsif` is written at the indentation of the `if` it continues.
    let head = match node.kind_str() {
        "elsif" => {
            format!("{indentation}{keyword} {condition}\n{indentation}{body}{consequence}\n")
        }
        _ => format!("{keyword} {condition}\n{indentation}{body}{consequence}\n"),
    };
    let tail = match node.field("alternative") {
        None => "end".to_owned(),
        Some(alternative) if alternative.kind_str() == "elsif" => {
            expanded(context, alternative, width, column)
        }
        Some(alternative) => format!(
            "{indentation}else\n{indentation}{body}{}\n{indentation}end",
            clause_source(context, alternative)
        ),
    };
    head + &tail
}

/// The source of everything a `then` or `else` clause holds.
fn clause_source(context: &RuleContext<'_>, clause: Node<'_>) -> String {
    let written = super::nodes::children(clause);
    match (written.first(), written.last()) {
        (Some(first), Some(last)) => context
            .source
            .slice(first.start_byte()..last.end_byte())
            .to_owned(),
        _ => "nil".to_owned(),
    }
}
