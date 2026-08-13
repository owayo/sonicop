use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// Every node kind that carries a condition upstream reads with `on_if` / `on_while` / `on_until`,
/// including the ternary and the post-loop forms.
const CONDITIONALS: &[&str] = &[
    "if",
    "unless",
    "elsif",
    "conditional",
    "while",
    "until",
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

/// `COMPARISON_OPERATORS`, which `correct_other` wraps rather than parenthesizing the arguments of.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "conditionals".to_owned());
    let locals = LocalVariables::new(context);
    let mut reported: HashSet<Range<usize>> = HashSet::new();
    if style == "always" {
        for node in context.nodes_of("binary") {
            report(context, &locals, node, &mut reported, offenses);
        }
        return;
    }
    for node in context.nodes_of_any(CONDITIONALS) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let mut stack = vec![condition];
        while let Some(current) = stack.pop() {
            if current.kind_str() == "binary" {
                report(context, &locals, current, &mut reported, offenses);
            }
            stack.extend(super::nodes::children(current));
        }
    }
}

fn report(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    reported: &mut HashSet<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    let Some(operator) = node.field("operator") else {
        return;
    };
    // `logical_operator?`: `&&` and `||` are already what the cop asks for.
    let alternate = match context.source.node_text(operator) {
        "and" => "&&",
        "or" => "||",
        _ => return,
    };
    if !reported.insert(operator.byte_range()) {
        return;
    }
    let current = context.source.node_text(operator);
    offenses.push(
        context
            .offense(
                format!("Use `{alternate}` instead of `{current}`."),
                operator.byte_range(),
            )
            .corrected_by_all(corrections(context, locals, node, operator, alternate)),
    );
}

fn corrections(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    operator: Node<'_>,
    alternate: &str,
) -> Vec<Edit> {
    let mut edits = Vec::new();
    let operands = [
        node.field("left"),
        node.field("right"),
    ];
    for child in operands.into_iter().flatten() {
        if is_send(context, locals, child) {
            correct_send(context, locals, child, node, &mut edits);
        } else if matches!(child.kind_str(), "return" | "next" | "break" | "yield")
            || matches!(child.kind_str(), "assignment" | "operator_assignment")
        {
            correct_other(context, child, node, &mut edits);
        }
    }
    edits.push(Edit {
        start: operator.start_byte(),
        end: operator.end_byte(),
        replacement: alternate.to_owned(),
        safe: true,
    });
    keep_operator_precedence(context, node, &mut edits);
    edits
}

/// Whether upstream's parser would have built a `send` here. A setter call is one too, however much
/// the grammar spells it as an assignment.
fn is_send(context: &RuleContext<'_>, locals: &LocalVariables<'_, '_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "call" | "element_reference" => true,
        "binary" => node
            .field("operator")
            .is_some_and(|operator| {
                super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        "unary" => node
            .field("operator")
            .is_some_and(|operator| {
                matches!(context.source.node_text(operator), "!" | "not")
                    || super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        "identifier" => !locals.is_lvar(node),
        // A `=~` the grammar read as an assignment is an operator call, and so is a setter.
        "assignment" => {
            super::nodes::is_match_assignment(node, context.source.text())
                || setter_target(node).is_some()
        }
        _ => false,
    }
}

/// `setter_method?`: an assignment written as a call is a `send` whose selector ends in `=`.
fn setter_target<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("left")
        .filter(|left| matches!(left.kind_str(), "call" | "element_reference"))
}

fn correct_send(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    parent: Node<'_>,
    edits: &mut Vec<Edit>,
) {
    if node.kind_str() == "unary"
        && let Some(operator) = node.field("operator")
    {
        match context.source.node_text(operator) {
            // `!x` can be corrected by descending into what is negated.
            "!" => {
                if let Some(operand) = node.field("operand")
                    && is_send(context, locals, operand)
                {
                    correct_send(context, locals, operand, node, edits);
                }
                return;
            }
            "not" => {
                correct_other(context, node, parent, edits);
                return;
            }
            _ => {}
        }
    }
    // A setter keeps its own parentheses around the whole assignment.
    if node.kind_str() == "assignment"
        && setter_target(node).is_some()
        && !super::nodes::is_match_assignment(node, context.source.text())
    {
        edits.push(insert(node.start_byte(), "("));
        edits.push(insert(node.end_byte(), ")"));
        return;
    }
    if is_comparison(context, node) {
        correct_other(context, node, parent, edits);
        return;
    }
    let Some((selector, last_argument)) = unparenthesized_call(context, node) else {
        return;
    };
    // `whitespace_before_arg`: the space between the selector and its first argument becomes the
    // opening parenthesis, unless a predicate was written straight against its argument.
    let source = context.source.node_text(node);
    let width = usize::from(!has_predicate_without_space(source));
    edits.push(Edit {
        start: selector.end_byte(),
        end: selector.end_byte() + width,
        replacement: "(".to_owned(),
        safe: true,
    });
    edits.push(insert(last_argument.end_byte(), ")"));
}

/// `/\?\S/`: a predicate whose argument follows it with no space at all.
fn has_predicate_without_space(source: &str) -> bool {
    let bytes = source.as_bytes();
    (0..bytes.len().saturating_sub(1))
        .any(|index| bytes[index] == b'?' && !bytes[index + 1].is_ascii_whitespace())
}

/// `correctable_send?`: a call written without parentheses that has arguments to wrap, and is not
/// an indexing.
fn unparenthesized_call<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    match node.kind_str() {
        "call" => {
            let selector = node.field("method")?;
            if context.source.node_text(selector) == "[]" {
                return None;
            }
            let arguments = node.field("arguments")?;
            if context.source.node_text(arguments).starts_with('(') {
                return None;
            }
            let last = super::nodes::children(arguments).last().copied()?;
            Some((selector, last))
        }
        // An operator call is never written with parentheses around its one argument.
        "binary" => Some((
            node.field("operator")?,
            node.field("right")?,
        )),
        // The `=~` the grammar split in two: the selector spans both characters, and the argument
        // is what the `~` was read as applying to.
        "assignment" if super::nodes::is_match_assignment(node, context.source.text()) => {
            let right = node.field("right")?;
            Some((
                right.field("operator")?,
                right.field("operand")?,
            ))
        }
        _ => None,
    }
}

fn is_comparison(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let selector = match node.kind_str() {
        "binary" => node.field("operator"),
        "call" => node.field("method"),
        _ => None,
    };
    selector
        .is_some_and(|selector| COMPARISON_OPERATORS.contains(&context.source.node_text(selector)))
}

fn correct_other(
    context: &RuleContext<'_>,
    node: Node<'_>,
    parent: Node<'_>,
    edits: &mut Vec<Edit>,
) {
    if node.kind_str() == "call"
        && node
            .field("arguments")
            .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
    {
        return;
    }
    // A bare `return` written as the right operand cannot be wrapped without changing what it
    // returns.
    if super::nodes::children(node).is_empty()
        && parent
            .field("right")
            .is_some_and(|right| right.id() == node.id())
    {
        return;
    }
    edits.push(insert(node.start_byte(), "("));
    edits.push(insert(node.end_byte(), ")"));
}

/// `keep_operator_precedence`: `&&` binds tighter than `||`, which `and` and `or` do not.
fn keep_operator_precedence(context: &RuleContext<'_>, node: Node<'_>, edits: &mut Vec<Edit>) {
    let kind = |node: Node<'_>| {
        (node.kind_str() == "binary")
            .then(|| node.field("operator"))
            .flatten()
            .map(|operator| context.source.node_text(operator))
    };
    let own = kind(node);
    if matches!(own, Some("or"))
        && node
            .parent()
            .and_then(kind)
            .is_some_and(|parent| matches!(parent, "and" | "&&"))
    {
        edits.push(insert(node.start_byte(), "("));
        edits.push(insert(node.end_byte(), ")"));
        return;
    }
    if matches!(own, Some("and"))
        && let Some(right) = node.field("right")
        && matches!(kind(right), Some("or" | "||"))
    {
        edits.push(insert(right.start_byte(), "("));
        edits.push(insert(right.end_byte(), ")"));
    }
}

fn insert(offset: usize, text: &str) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement: text.to_owned(),
        safe: true,
    }
}
