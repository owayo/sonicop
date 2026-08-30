use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::support::correction_parses;
use crate::rules::node_ext::NodeExt;

/// The node kinds upstream's `on_if` is called for. `elsif` is one there too -- it is an `if` node
/// whose keyword happens to be `elsif` -- and the cop's first guard drops it.
const CONDITIONALS: &[&str] = &["if", "unless", "elsif", "if_modifier", "unless_modifier"];

const MODIFIERS: &[&str] = &["if_modifier", "unless_modifier"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_modifier: bool = context.setting("AllowModifier").unwrap_or(false);
    // `ignore_node(if_branch)`: the outer conditional's rewrite already covers the inner one, so
    // the inner one is reported without a correction of its own.
    let mut ignored: Vec<usize> = Vec::new();

    for node in context.nodes_of_any(CONDITIONALS) {
        let Some(if_branch) = offending_branch(context, node, allow_modifier) else {
            continue;
        };
        let edits = autocorrect(context, node, if_branch);
        if edits.is_empty() || !correction_parses(context, &edits) {
            continue;
        }
        let Some(inner_keyword) = keyword(if_branch) else {
            continue;
        };
        let message = format!(
            "Consider merging nested conditions into outer `{}` conditions.",
            keyword(node).map_or("if", |range| context.source.slice(range))
        );
        let offense = context.offense(message, inner_keyword);
        if ignored.contains(&node.id()) {
            offenses.push(offense);
            continue;
        }
        offenses.push(offense.corrected_by_all(edits));
        ignored.push(if_branch.id());
    }
}

/// The `if`, `unless` or `elsif` token a conditional opens with.
fn keyword(node: Node<'_>) -> Option<Range<usize>> {
    super::conditional::token(node, &["if", "unless", "elsif"]).map(|token| token.byte_range())
}

fn is_if(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "if" | "elsif" | "if_modifier")
}

fn condition_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("condition")
}

/// `offending_conditional?`, returning the branch it found rather than a flag.
fn offending_branch<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    allow_modifier: bool,
) -> Option<Node<'tree>> {
    // `node.ternary?` is a node kind of its own here, so only `elsif?` and `else?` are left.
    if node.kind_str() == "elsif" || has_else(node) {
        return None;
    }
    let branch = if_branch(node)?;
    if use_variable_assignment_in_condition(context, node, branch) {
        return None;
    }
    // `offending_branch?`.
    if !CONDITIONALS.contains(&branch.kind_str()) || branch.kind_str() == "elsif" || has_else(branch) {
        return None;
    }
    let modifier = is_modifier(node) || is_modifier(branch);
    if modifier && allow_modifier {
        return None;
    }
    Some(branch)
}

fn is_modifier(node: Node<'_>) -> bool {
    MODIFIERS.contains(&node.kind_str())
}

/// `node.else?`: whether the conditional carries an `else` clause. An `elsif` counts, because the
/// parser points the outer conditional's `else` location at the `elsif` keyword.
fn has_else(node: Node<'_>) -> bool {
    super::nodes::children(node)
        .into_iter()
        .any(|child| matches!(child.kind_str(), "else" | "elsif"))
}

/// `node.if_branch`: the body the conditional runs when it is taken, which for an `unless` is the
/// clause before its `else` all the same.
fn if_branch<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if is_modifier(node) {
        return node.field("body");
    }
    // A `then` clause holding more than one statement is a `begin` upstream, which is no `if`.
    super::conditional::body_of(node.field("consequence")?).single()
}

/// `use_variable_assignment_in_condition?`: merging would move the inner condition ahead of the
/// assignment its own name comes from.
fn use_variable_assignment_in_condition(
    context: &RuleContext<'_>,
    node: Node<'_>,
    branch: Node<'_>,
) -> bool {
    let Some(condition) = condition_of(node) else {
        return false;
    };
    let Some(inner) = condition_of(branch) else {
        return false;
    };
    let inner_source = context.source.node_text(inner);
    super::conditional::descendants(condition, context)
        .into_iter()
        .filter(|descendant| descendant.kind_str() == "assignment")
        .filter_map(|assignment| assignment.field("left"))
        // `children.first.to_s`: only a plain variable's name comes out as its own source. A
        // constant yields its namespace, which is empty for a bare one, and a multiple assignment
        // or a shorthand one yields the s-expression of a node, which no condition is written as.
        .filter(|left| {
            matches!(
                left.kind_str(),
                "identifier" | "instance_variable" | "global_variable" | "class_variable"
            )
        })
        .any(|left| context.source.node_text(left) == inner_source)
}

/// The rewrite that merges the two conditions, as the corrector schedules it.
fn autocorrect(context: &RuleContext<'_>, node: Node<'_>, if_branch: Node<'_>) -> Vec<Edit> {
    if is_modifier(node) {
        return autocorrect_outer_condition_modify_form(context, node, if_branch);
    }
    let mut edits = correct_node(context, node);
    if is_modifier(if_branch) {
        edits.extend(correct_for_guard_condition_style(context, node, if_branch));
    } else {
        edits.extend(correct_for_basic_condition_style(context, node, if_branch));
        edits.extend(correct_for_comment(context, node, if_branch));
    }
    edits
}

/// `correct_node`: the keyword becomes `if` and the condition becomes the one the merge chains.
fn correct_node(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Edit> {
    let mut edits = Vec::new();
    if !is_if(node)
        && let Some(range) = keyword(node)
    {
        edits.push(Edit {
            start: range.start,
            end: range.end,
            replacement: "if".to_owned(),
            safe: true,
        });
    }
    if let Some(condition) = condition_of(node) {
        edits.push(Edit {
            start: condition.start_byte(),
            end: condition.end_byte(),
            replacement: chainable_condition(context, node),
            safe: true,
        });
    }
    edits
}

/// `correct_for_guard_condition_style`: the inner modifier moves up into the outer condition.
fn correct_for_guard_condition_style(
    context: &RuleContext<'_>,
    node: Node<'_>,
    if_branch: Node<'_>,
) -> Vec<Edit> {
    let (Some(condition), Some(inner), Some(inner_keyword)) = (
        condition_of(node),
        condition_of(if_branch),
        keyword(if_branch),
    ) else {
        return Vec::new();
    };
    let text = context.source.text();
    let start = super::ranges::extended_left(text, inner_keyword.start, false);
    let end = super::ranges::extended_right(text, inner.end_byte(), false);
    vec![
        Edit {
            start: condition.end_byte(),
            end: condition.end_byte(),
            replacement: format!(" && {}", chainable_condition(context, if_branch)),
            safe: true,
        },
        Edit {
            start,
            end,
            replacement: String::new(),
            safe: true,
        },
    ]
}

/// `correct_for_basic_condition_style`: the two conditions join and the inner `end` goes.
fn correct_for_basic_condition_style(
    context: &RuleContext<'_>,
    node: Node<'_>,
    if_branch: Node<'_>,
) -> Vec<Edit> {
    let (Some(condition), Some(inner)) = (condition_of(node), condition_of(if_branch)) else {
        return Vec::new();
    };
    let (Some(outer_end), Some(inner_end)) = (end_token(node), end_token(if_branch)) else {
        return Vec::new();
    };
    let mut edits = vec![
        Edit {
            start: condition.end_byte(),
            end: inner.start_byte(),
            replacement: " && ".to_owned(),
            safe: true,
        },
        Edit {
            start: inner.start_byte(),
            end: inner.end_byte(),
            replacement: chainable_condition(context, if_branch),
            safe: true,
        },
    ];
    // Only the outer `end` is left over, and it takes its whole line with it unless the inner one
    // shares it.
    let same_line = context.source.line_column(outer_end.start).0
        == context.source.line_column(inner_end.start).0;
    let range = if same_line {
        outer_end
    } else {
        whole_lines(context, outer_end)
    };
    edits.push(Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    });
    edits
}

/// `autocorrect_outer_condition_modify_form`: the outer modifier moves down into the inner one.
fn autocorrect_outer_condition_modify_form(
    context: &RuleContext<'_>,
    node: Node<'_>,
    if_branch: Node<'_>,
) -> Vec<Edit> {
    let (Some(condition), Some(inner), Some(outer_keyword)) =
        (condition_of(node), condition_of(if_branch), keyword(node))
    else {
        return Vec::new();
    };
    let mut edits = correct_node(context, if_branch);
    edits.push(Edit {
        start: inner.start_byte(),
        end: inner.start_byte(),
        replacement: format!("{} && ", chainable_condition(context, node)),
        safe: true,
    });
    let text = context.source.text();
    edits.push(Edit {
        start: super::ranges::extended_left(text, outer_keyword.start, false),
        end: super::ranges::extended_right(text, condition.end_byte(), false),
        replacement: String::new(),
        safe: true,
    });
    edits
}

/// `correct_for_comment`: a comment written above the inner condition would end up commenting out
/// the merged one, so it moves above the outer keyword instead.
fn correct_for_comment(
    context: &RuleContext<'_>,
    node: Node<'_>,
    if_branch: Node<'_>,
) -> Vec<Edit> {
    let (Some(outer), Some(inner), Some(outer_keyword)) =
        (condition_of(node), condition_of(if_branch), keyword(node))
    else {
        return Vec::new();
    };
    let limit = context.source.line_column(inner.start_byte()).0;
    // `ast_with_comments[if_branch]`: the associator hands each comment to the first node it
    // reaches whose source starts at or after the comment's end, walking the tree in source order.
    // The node before the branch is the outer condition, so the comments that land on the branch
    // are the ones written between the two -- less any that merely decorate the line the outer
    // condition ends on, which the condition takes for itself first.
    let decorated = context.source.line_column(outer.end_byte()).0;
    let comments: Vec<&str> = context
        .comment_ranges()
        .iter()
        .filter(|comment| {
            comment.start >= outer.end_byte() && comment.end <= if_branch.start_byte()
        })
        .filter(|comment| {
            let line = context.source.line_column(comment.start).0;
            line != decorated && line < limit
        })
        .map(|comment| context.source.slice(comment.clone()))
        .collect();
    if comments.is_empty() {
        return Vec::new();
    }
    vec![Edit {
        start: outer_keyword.start,
        end: outer_keyword.start,
        replacement: format!("{}\n", comments.join("\n")),
        safe: true,
    }]
}

fn end_token(node: Node<'_>) -> Option<Range<usize>> {
    super::conditional::token(node, &["end"]).map(|token| token.byte_range())
}

/// `range_by_whole_lines(range, include_final_newline: true)`.
fn whole_lines(context: &RuleContext<'_>, range: Range<usize>) -> Range<usize> {
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |newline| range.end + newline + 1);
    start..end
}

/// `chainable_condition`: the condition as it reads once chained onto the other one, negated when
/// the conditional it came from was an `unless`.
fn chainable_condition(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let Some(condition) = condition_of(node) else {
        return String::new();
    };
    let wrapped = add_parentheses_if_needed(context, condition);
    if is_if(node) {
        return wrapped;
    }
    match is_and(context, condition) {
        true => format!("!({wrapped})"),
        false => format!("!{wrapped}"),
    }
}

fn is_and(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "&&" | "and"))
}

fn is_or(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "||" | "or"))
}

/// `add_parentheses_if_needed`.
fn add_parentheses_if_needed(context: &RuleContext<'_>, condition: Node<'_>) -> String {
    let source = context.source.node_text(condition).to_owned();
    if !add_parentheses(context, condition) {
        return source;
    }
    if parenthesize_method(context, condition) {
        return parenthesized_method_arguments(context, condition);
    }
    if is_and(context, condition) {
        return parenthesized_and(context, condition);
    }
    format!("({source})")
}

/// `add_parentheses?`, read on the call itself where a block would otherwise hide it.
fn add_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    // `node.assignment?`, which for a call means `setter_method?` -- and the grammar spells every
    // one of those, `a.b = 1` included, as an assignment.
    if matches!(node.kind_str(), "assignment" | "operator_assignment") || is_or(context, node) {
        return true;
    }
    if assignment_in_and(context, node) {
        return true;
    }
    let Some((arguments, parenthesized, prefix_not)) = call_shape(context, node) else {
        return false;
    };
    (arguments && !parenthesized) || prefix_not
}

/// `assignment_in_and?`.
fn assignment_in_and(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    is_and(context, node)
        && super::conditional::descendants(node, context)
            .into_iter()
            .skip(1)
            .any(|descendant| matches!(descendant.kind_str(), "assignment" | "operator_assignment"))
}

/// Whether the node is a call, and if so whether it takes arguments, holds them in parentheses, and
/// is the `not` spelling of `!`.
fn call_shape(context: &RuleContext<'_>, node: Node<'_>) -> Option<(bool, bool, bool)> {
    match node.kind_str() {
        "call" => {
            let arguments = node.field("arguments");
            let parenthesized =
                arguments.is_some_and(|list| context.source.node_text(list).starts_with('('));
            let any = arguments.is_some_and(|list| !super::nodes::children_in(list, context).is_empty());
            Some((any, parenthesized, false))
        }
        // `a[0]` is `(send a :[] 0)`: an argument, and no parentheses around it.
        "element_reference" => Some((true, false, false)),
        // A binary operator is a call with the right operand as its only argument, unless it is
        // one of the four the parser builds an `and` or an `or` from.
        "binary" if !is_and(context, node) && !is_or(context, node) => Some((true, false, false)),
        "unary" => {
            let operator = node.field("operator")?;
            let text = context.source.node_text(operator);
            // `defined?` is a keyword rather than a call, and the two unary sign operators and `!`
            // take no argument at all.
            match text {
                "defined?" => None,
                "not" => Some((false, false, true)),
                _ => Some((false, false, false)),
            }
        }
        // A bare name is a call with no arguments, whether or not a local variable shadows it.
        "identifier" | "constant" | "scope_resolution" | "self" => Some((false, false, false)),
        _ => None,
    }
}

/// `parenthesize_method?`.
fn parenthesize_method(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    // `node.call_type?`: a call carrying a block is a `block` node upstream, not a `send`, so this
    // branch is not taken for it and the **whole thing** gets the parentheses. The grammar hangs
    // the block off the call, so asking only about the kind takes the argument-list branch and
    // writes `ok?(bar do ... end)` where upstream writes `(ok? bar do ... end)`.
    if node.field("block").is_some() {
        return false;
    }
    let Some((arguments, parenthesized, _)) = call_shape(context, node) else {
        return false;
    };
    if !arguments || parenthesized {
        return false;
    }
    node.field("method")
        .is_some_and(|method| !super::nodes::is_operator_method(context.source.node_text(method)))
}

/// `parenthesized_method_arguments`: `foo bar` becomes `foo(bar)`.
fn parenthesized_method_arguments(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let (Some(selector), Some(arguments)) = (
        node.field("method"),
        node.field("arguments"),
    ) else {
        return context.source.node_text(node).to_owned();
    };
    let call = context.source.slice(node.start_byte()..selector.end_byte());
    let first = super::nodes::children_in(arguments, context)
        .first()
        .map_or(arguments.start_byte(), Node::start_byte);
    let rest = context.source.slice(first..node.end_byte());
    format!("{call}({rest})")
}

/// `parenthesized_and`: only a clause that assigns needs parentheses of its own, since every other
/// clause reads the same once the conditions are chained.
fn parenthesized_and(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let (Some(left), Some(operator), Some(right)) = (
        node.field("left"),
        node.field("operator"),
        node.field("right"),
    ) else {
        return context.source.node_text(node).to_owned();
    };
    let text = context.source.text();
    // `range_with_surrounding_space(node.loc.operator, whitespace: true)`: the `whitespace` stage
    // runs **after** the newline stage, so it reaches the indentation of the line the right-hand
    // clause sits on. Stopping at the newline drops that indentation and moves the clause to
    // column 0.
    let spaced = context.source.slice(
        crate::rules::support::final_pos(text, operator.start_byte(), false, false, true, true)
            ..crate::rules::support::final_pos(text, operator.end_byte(), true, false, true, true),
    );
    format!(
        "{}{}{}",
        context.source.node_text(left),
        spaced,
        parenthesized_and_clause(context, right)
    )
}

fn parenthesized_and_clause(context: &RuleContext<'_>, node: Node<'_>) -> String {
    if is_and(context, node) {
        return parenthesized_and(context, node);
    }
    match matches!(node.kind_str(), "assignment" | "operator_assignment") {
        true => format!("({})", context.source.node_text(node)),
        false => context.source.node_text(node).to_owned(),
    }
}
