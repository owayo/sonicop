//! `EmptyLinesAroundBody`, the mixin the five `Layout/EmptyLinesAround*Body` cops are built on.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The body of a construct as upstream's parser hands it over: absent, one node, or the `begin`
/// that holds a statement list.
pub(super) enum Body<'tree> {
    None,
    Single(Node<'tree>),
    Begin(Vec<Node<'tree>>),
}

/// One construct to inspect: the lines its body is framed by, and that body.
pub(super) struct Target<'tree> {
    /// `adjusted_first_line || node.source_range.first_line`.
    pub(super) first_line: usize,
    pub(super) last_line: usize,
    pub(super) single_line: bool,
    pub(super) body: Body<'tree>,
}

pub(super) fn check(
    context: &RuleContext<'_>,
    kind: &str,
    style: &str,
    targets: Vec<Target<'_>>,
    offenses: &mut Vec<Offense>,
) {
    // `add_offense` keeps a set of the ranges it has reported, so an empty body whose one blank
    // line is both its beginning and its end is reported once, under the first message.
    let mut reported = HashSet::new();
    for target in targets {
        // `valid_body_style?`: an empty body is left alone unless blank lines are forbidden.
        if matches!(target.body, Body::None) && style != "no_empty_lines" {
            continue;
        }
        if target.single_line {
            continue;
        }
        match style {
            "empty_lines_except_namespace" => {
                let inner = match namespace(&target.body) {
                    true => "no_empty_lines",
                    false => "empty_lines",
                };
                check_both(context, kind, inner, &target, &mut reported, offenses);
            }
            "empty_lines_special" => check_special(context, kind, &target, &mut reported, offenses),
            _ => check_both(context, kind, style, &target, &mut reported, offenses),
        }
    }
}

fn check_both(
    context: &RuleContext<'_>,
    kind: &str,
    style: &str,
    target: &Target<'_>,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    let (beginning, ending) = match style {
        "beginning_only" => ("empty_lines", "no_empty_lines"),
        "ending_only" => ("no_empty_lines", "empty_lines"),
        _ => (style, style),
    };
    check_beginning(
        context,
        kind,
        beginning,
        target.first_line,
        reported,
        offenses,
    );
    check_ending(context, kind, ending, target.last_line, reported, offenses);
}

/// `check_empty_lines_special`.
fn check_special(
    context: &RuleContext<'_>,
    kind: &str,
    target: &Target<'_>,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    if matches!(target.body, Body::None) {
        return;
    }
    if namespace(&target.body) {
        check_both(context, kind, "no_empty_lines", target, reported, offenses);
        return;
    }
    if first_child_requires_empty_line(&target.body) {
        check_beginning(
            context,
            kind,
            "empty_lines",
            target.first_line,
            reported,
            offenses,
        );
    } else {
        check_beginning(
            context,
            kind,
            "no_empty_lines",
            target.first_line,
            reported,
            offenses,
        );
        check_deferred(context, &target.body, reported, offenses);
    }
    check_ending(
        context,
        kind,
        "empty_lines",
        target.last_line,
        reported,
        offenses,
    );
}

fn check_beginning(
    context: &RuleContext<'_>,
    kind: &str,
    style: &str,
    first_line: usize,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    // Upstream indexes `processed_source.lines` with the one-based line number, which lands on the
    // line after it.
    check_source(
        context,
        kind,
        style,
        first_line + 1,
        "beginning",
        reported,
        offenses,
    );
}

fn check_ending(
    context: &RuleContext<'_>,
    kind: &str,
    style: &str,
    last_line: usize,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    if last_line < 2 {
        return;
    }
    check_source(
        context,
        kind,
        style,
        last_line - 1,
        "end",
        reported,
        offenses,
    );
}

fn check_source(
    context: &RuleContext<'_>,
    kind: &str,
    style: &str,
    line: usize,
    location: &str,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    let empty = is_blank(context, line);
    match style {
        "no_empty_lines" if empty => {
            let range = line_head(context, line);
            if !reported.insert((range.start, range.end)) {
                return;
            }
            offenses.push(
                context
                    .offense(
                        format!("Extra empty line detected at {kind} body {location}."),
                        range.clone(),
                    )
                    .corrected_by(Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
        "empty_lines" if !empty => {
            // A missing line at the end is reported on the closing line rather than on the last
            // line of the body.
            let range = line_head(context, if location == "end" { line + 1 } else { line });
            if !reported.insert((range.start, range.end)) {
                return;
            }
            offenses.push(
                context
                    .offense(
                        format!("Empty line missing at {kind} body {location}."),
                        range.clone(),
                    )
                    .corrected_by(Edit {
                        start: range.start,
                        end: range.start,
                        replacement: "\n".to_owned(),
                        safe: true,
                    }),
            );
        }
        _ => {}
    }
}

/// `check_deferred_empty_line`: the first definition inside the body wants a blank line before it
/// even when the body itself must open right away.
fn check_deferred(
    context: &RuleContext<'_>,
    body: &Body<'_>,
    reported: &mut HashSet<(usize, usize)>,
    offenses: &mut Vec<Offense>,
) {
    let Some(node) = first_empty_line_required_child(body) else {
        return;
    };
    // `previous_line_ignoring_comments`: the nearest line above the definition that is not a
    // comment, or the first line of the file when every line above it is one.
    let start = node.start_position().row + 1;
    let mut previous = 1;
    for candidate in (1..start).rev() {
        if !is_comment_line(context, candidate) {
            previous = candidate;
            break;
        }
    }
    if is_blank(context, previous) {
        return;
    }
    let range = line_head(context, previous + 1);
    if !reported.insert((range.start, range.end)) {
        return;
    }
    let kind = match node.kind_str() {
        "method" => "def",
        "singleton_method" => "defs",
        "class" => "class",
        "module" => "module",
        _ => "send",
    };
    offenses.push(
        context
            .offense(
                format!("Empty line missing before first {kind} definition"),
                range.clone(),
            )
            .corrected_by(Edit {
                start: range.start,
                end: range.start,
                replacement: "\n".to_owned(),
                safe: true,
            }),
    );
}

/// `source_range(buffer, line, 0)`: column zero of the line, one character long.
fn line_head(context: &RuleContext<'_>, line: usize) -> std::ops::Range<usize> {
    let start = context.source.line_start(line);
    let end = (start + 1).min(context.source.text().len());
    start..end
}

fn is_blank(context: &RuleContext<'_>, line: usize) -> bool {
    line <= context.source.line_count()
        && context.source.line(line).trim_end_matches('\n').is_empty()
}

fn is_comment_line(context: &RuleContext<'_>, line: usize) -> bool {
    context.source.line(line).trim_start().starts_with('#')
}

/// `namespace?(body, with_one_child: true)`: a body of exactly one class or module.
fn namespace(body: &Body<'_>) -> bool {
    match body {
        Body::Single(node) => matches!(node.kind_str(), "class" | "module"),
        _ => false,
    }
}

fn first_child_requires_empty_line(body: &Body<'_>) -> bool {
    match body {
        Body::None => false,
        Body::Single(node) => requires_empty_line(*node),
        Body::Begin(nodes) => nodes.first().is_some_and(|node| requires_empty_line(*node)),
    }
}

fn first_empty_line_required_child<'tree>(body: &Body<'tree>) -> Option<Node<'tree>> {
    match body {
        Body::None => None,
        Body::Single(node) => requires_empty_line(*node).then_some(*node),
        Body::Begin(nodes) => nodes
            .iter()
            .copied()
            .find(|node| requires_empty_line(*node)),
    }
}

/// `{any_def class module (send nil? {:private :protected :public})}`.
fn requires_empty_line(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "method" | "singleton_method" | "class" | "module" | "identifier"
    ) && (node.kind_str() != "identifier" || is_bare_access_modifier(node))
}

fn is_bare_access_modifier(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind_str(),
            "body_statement" | "block_body" | "program"
        )
    })
}

/// The statements upstream's parser puts in a body, folded into the shape it gives them.
pub(super) fn body_of<'tree>(container: Option<Node<'tree>>) -> Body<'tree> {
    let Some(container) = container else {
        return Body::None;
    };
    let mut cursor = container.walk();
    let statements: Vec<Node<'tree>> = container
        .named_children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind_str(),
                "comment" | "heredoc_body" | "empty_statement"
            )
        })
        .collect();
    // A `rescue` or `ensure` clause takes the statements over upstream, leaving the body a single
    // node that is neither a definition nor a namespace.
    if let Some(clause) = statements
        .iter()
        .find(|child| matches!(child.kind_str(), "rescue" | "ensure" | "else"))
    {
        return Body::Single(*clause);
    }
    match statements.len() {
        0 => Body::None,
        1 => Body::Single(statements[0]),
        _ => Body::Begin(statements),
    }
}

/// The `body_statement` or `block_body` a construct holds, if it has one.
pub(super) fn body_container<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| matches!(child.kind_str(), "body_statement" | "block_body"))
}
