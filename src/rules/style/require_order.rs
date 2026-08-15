//! `Style/RequireOrder`: a run of `require`s that is not in alphabetical order.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_string, string_text};

/// `RESTRICT_ON_SEND`.
const REQUIRE_METHODS: &[&str] = &["require", "require_relative"];

/// The conditional forms a `require` may be written under and still count as one statement.
const MODIFIERS: &[&str] = &["if_modifier", "unless_modifier"];

/// The block forms, which `not_modifier_form?` stops the search at.
const BLOCK_CONDITIONALS: &[&str] = &["if", "unless"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if !is_require(call, context) {
            continue;
        }
        let Some(parent) = call.parent() else {
            continue;
        };
        if BLOCK_CONDITIONALS.contains(&parent.kind_str()) {
            continue;
        }
        let Some(previous) = find_previous_older_sibling(context, call, parent) else {
            continue;
        };
        let name = method_name(call, context).unwrap_or_default();
        let moved = match MODIFIERS.contains(&parent.kind_str()) {
            true => parent,
            false => call,
        };
        let from = with_comments_and_lines(context, moved);
        let to = with_comments_and_lines(context, previous);
        offenses.push(
            context
                .offense(
                    format!("Sort `{name}` in alphabetical order."),
                    call.byte_range(),
                )
                .corrections_anchored_at(to.clone())
                .corrected_by_all([
                    Edit {
                        start: from.start,
                        end: from.end,
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: to.start,
                        end: to.start,
                        replacement: context.source.slice(from.clone()).to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `(send nil? {:require :require_relative} _)` with an argument.
fn is_require(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    call.field("receiver").is_none()
        && method_name(call, context).is_some_and(|name| REQUIRE_METHODS.contains(&name))
        && !arguments(call).is_empty()
}

fn method_name<'a>(call: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    (call.kind_str() == "call")
        .then(|| call.field("method"))?
        .map(|name| context.source.node_text(name))
}

/// `find_previous_older_sibling`: the nearest `require` above that this one sorts before, with the
/// search stopping at anything that is not a `require` of the same kind.
fn find_previous_older_sibling<'tree>(
    context: &RuleContext<'_>,
    call: Node<'tree>,
    parent: Node<'tree>,
) -> Option<Node<'tree>> {
    // `search_node`: a `require` written under a modifier is looked for beside the modifier.
    let search = match MODIFIERS.contains(&parent.kind_str()) {
        true => parent,
        false => call,
    };
    let container = search.parent()?;
    let statements = super::nodes::children(container);
    let position = statements
        .iter()
        .position(|statement| statement.id() == search.id())?;
    let name = method_name(call, context)?;
    let value = argument_string(call, context)?;
    for sibling in statements[..position].iter().rev() {
        // `sibling_node`: a block-form conditional ends the run; a modifier hands out the
        // `require` written inside it.
        let sibling = sibling_node(*sibling, context)?;
        if method_name(sibling, context) != Some(name)
            || sibling.field("receiver").is_some()
            || arguments(sibling).is_empty()
            || !in_same_section(context, sibling, call)
        {
            return None;
        }
        let other = argument_string(sibling, context)?;
        if value < other {
            return Some(sibling);
        }
    }
    None
}

/// `sibling_node`.
fn sibling_node<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if BLOCK_CONDITIONALS.contains(&node.kind_str()) {
        return None;
    }
    if !MODIFIERS.contains(&node.kind_str()) {
        return Some(node);
    }
    // `if_inside_only_require`: the branch has to be nothing but a `require`.
    let body = node.field("body")?;
    is_require(body, context).then_some(body)
}

/// `node.first_argument.value` when it is a string literal.
fn argument_string<'a>(call: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let call_arguments = arguments(call);
    let first = call_arguments.first()?.first();
    is_string(first, context).then(|| string_text(first, context))
}

/// `in_same_section?`: nothing between the two is a blank line.
fn in_same_section(context: &RuleContext<'_>, sibling: Node<'_>, call: Node<'_>) -> bool {
    let range = sibling.start_byte()..call.end_byte().max(sibling.start_byte());
    !context.source.slice(range).contains("\n\n")
}

/// `range_with_comments_and_lines`: the whole lines the statement and the comments written above
/// it sit on, with the newline that ends the last of them.
fn with_comments_and_lines(context: &RuleContext<'_>, node: Node<'_>) -> Range<usize> {
    let source = context.source;
    let start = leading_comments(context, node)
        .first()
        .map_or(node.start_byte(), |comment| comment.start);
    let (first, _) = source.line_column(start);
    let (last, _) = source.line_column(node.end_byte());
    source.line_start(first)..source.line_range(last).end
}

/// The comments `ast_with_comments` hands the node: the run written on lines of their own directly
/// above it.
fn leading_comments(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Range<usize>> {
    let source = context.source;
    let (line, column) = source.line_column(node.start_byte());
    if !source.line(line)[..column - 1].trim().is_empty() {
        return Vec::new();
    }
    let mut comments = Vec::new();
    for above in (1..line).rev() {
        let text = source.line(above);
        if text.trim().is_empty() {
            continue;
        }
        let start = source.line_start(above) + (text.len() - text.trim_start().len());
        let Some(comment) = context
            .comment_ranges()
            .iter()
            .find(|comment| comment.start == start)
        else {
            break;
        };
        comments.push(comment.clone());
    }
    comments.reverse();
    comments
}
