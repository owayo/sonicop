//! `Style/IfInsideElse`: an `if` that fills an `else` is an `elsif`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

const MSG: &str = "Convert `if` nested inside `else` to `elsif`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_modifier: bool = context.setting("AllowIfModifier").unwrap_or(false);
    // `ignore_node`: an `if` written inside one already being rewritten waits for the next pass.
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["if", "elsif"]) {
        let Some(alternative) = node.field("alternative") else {
            continue;
        };
        if alternative.kind_str() != "else" {
            continue;
        }
        let statements = super::nodes::children(alternative);
        let [inner] = statements.as_slice() else {
            continue;
        };
        // `else_branch.if?`: an `unless` written in an `else` cannot become an `elsif`.
        if !matches!(inner.kind_str(), "if" | "if_modifier") {
            continue;
        }
        let modifier = inner.kind_str() == "if_modifier";
        if (allow_modifier && modifier)
            || (!modifier && comments_between_else_and_if(context, alternative, *inner))
        {
            continue;
        }
        let Some(keyword) = keyword(*inner) else {
            continue;
        };
        // **The offense goes when its own correction clobbers itself.** For an `if` written with
        // neither `then` nor the modifier form, `correct_to_elsif_from_if_inside_else_form`
        // replaces the condition's range and removes the whole line the branch sits on -- the same
        // line, when the two were separated by a `;`. The rewriter raises, and RuboCop drops the
        // offense along with the correction.
        if !modifier && !has_then(*inner) && branch_shares_keyword_line(*inner, keyword) {
            continue;
        }
        let offense = context.offense(MSG, keyword.byte_range());
        let nested = ignored
            .iter()
            .any(|outer| outer.start <= node.start_byte() && node.end_byte() <= outer.end);
        offenses.push(match nested {
            true => offense,
            false => {
                ignored.push(node.byte_range());
                match autocorrect(context, alternative, *inner, keyword) {
                    Some(edits) => offense.corrected_by_all(edits),
                    None => offense,
                }
            }
        });
    }
}

/// Whether the branch stands on the line the `if` keyword was written on. The grammar's `then`
/// node starts at the line break, so it is the first statement inside it that answers this.
fn branch_shares_keyword_line(node: Node<'_>, keyword: Node<'_>) -> bool {
    node.field("consequence")
        .and_then(|consequence| super::nodes::children(consequence).first().copied())
        .is_some_and(|first| first.start_position().row == keyword.start_position().row)
}

fn autocorrect(
    context: &RuleContext<'_>,
    alternative: Node<'_>,
    inner: Node<'_>,
    keyword: Node<'_>,
) -> Option<Vec<Edit>> {
    // `if x then y end` is written out over lines first; the pass after that makes it an `elsif`.
    if has_then(inner) {
        return Some(vec![Edit {
            start: inner.start_byte(),
            end: inner.end_byte(),
            replacement: if_then_replacement(context, inner, Some(String::new())),
            safe: true,
        }]);
    }
    let condition = inner.field("condition")?;
    let else_keyword = alternative.child(0)?;
    let mut edits = vec![Edit {
        start: else_keyword.start_byte(),
        end: else_keyword.end_byte(),
        replacement: format!("elsif {}", context.source.node_text(condition)),
        safe: true,
    }];
    if inner.kind_str() == "if_modifier" {
        // `correct_to_elsif_from_modifier_form`: the condition moves to the `elsif`, so what was
        // written after the body goes.
        let body = inner.field("body")?;
        edits.push(remove(body.end_byte()..condition.end_byte()));
        return Some(edits);
    }
    let condition_range = keyword.start_byte()..condition.end_byte();
    match branch_range(context, inner, condition) {
        Some(branch) => {
            edits.push(Edit {
                start: condition_range.start,
                end: condition_range.end,
                replacement: context.source.slice(branch.clone()).to_owned(),
                safe: true,
            });
            edits.push(remove(support::whole_lines(branch, context)));
        }
        None => edits.push(remove(support::whole_lines(condition_range, context))),
    }
    edits.push(remove(condition.byte_range()));
    let end = end_keyword(inner)?;
    edits.push(remove(support::whole_lines(end.byte_range(), context)));
    Some(edits)
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// `range_with_comments(node.if_branch)`: the body of the inner `if`, together with the comments
/// the parser hands it -- the ones written between the condition and the body, and the one
/// trailing the body's last line.
fn branch_range(
    context: &RuleContext<'_>,
    inner: Node<'_>,
    condition: Node<'_>,
) -> Option<Range<usize>> {
    let consequence = inner.field("consequence")?;
    let statements = super::nodes::children(consequence);
    let first = statements.first()?;
    let last = statements.last()?;
    let mut range = first.start_byte()..last.end_byte();
    // A comment attaches to the statement it precedes rather than to the `begin` upstream wraps
    // several statements in, so a branch that wrote more than one carries none of them.
    if statements.len() > 1 {
        return Some(range);
    }
    let last_line = context.source.line_column(range.end).0;
    for comment in context.comment_ranges() {
        let leading = comment.start > condition.end_byte() && comment.end <= range.start;
        let trailing =
            comment.start >= range.end && context.source.line_column(comment.start).0 == last_line;
        if leading {
            range.start = range.start.min(comment.start);
        } else if trailing {
            range.end = range.end.max(comment.end);
        }
    }
    Some(range)
}

/// `comments_between_else_and_if?`.
fn comments_between_else_and_if(
    context: &RuleContext<'_>,
    alternative: Node<'_>,
    inner: Node<'_>,
) -> bool {
    let Some(else_keyword) = alternative.child(0) else {
        return false;
    };
    context.comment_ranges().iter().any(|comment| {
        comment.start > else_keyword.end_byte() && comment.start < inner.start_byte()
    })
}

/// The `if` of the conditional, which is what the offense points at.
fn keyword<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.kind_str() == "if")
}

fn end_keyword<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
    children
        .into_iter()
        .rev()
        .find(|child| !child.is_named() && child.kind_str() == "end")
}

/// `then?`: the branch was introduced with the keyword rather than a line break.
fn has_then(node: Node<'_>) -> bool {
    node.field("consequence")
        .and_then(|consequence| consequence.child(0))
        .is_some_and(|first| !first.is_named() && first.kind_str() == "then")
}

/// `IfThenCorrector#replacement`: the same conditional written over lines.
fn if_then_replacement(
    context: &RuleContext<'_>,
    node: Node<'_>,
    body_indent: Option<String>,
) -> String {
    let indentation = " ".repeat(context.source.line_column(node.start_byte()).1 - 1);
    if_then_written(
        context,
        node,
        &indentation,
        &body_indent.unwrap_or_default(),
    )
}

fn if_then_written(
    context: &RuleContext<'_>,
    node: Node<'_>,
    indentation: &str,
    body_indent: &str,
) -> String {
    let keyword = match node.kind_str() {
        "elsif" => "elsif",
        "unless" => "unless",
        _ => "if",
    };
    let condition = node
        .field("condition")
        .map_or_else(String::new, |condition| {
            context.source.node_text(condition).to_owned()
        });
    let body = node
        .field("consequence")
        .and_then(|consequence| statements_source(context, consequence))
        .unwrap_or_else(|| "nil".to_owned());
    // An `elsif` is written at the level of the `if` it continues rather than one deeper.
    let leading = match node.kind_str() == "elsif" {
        true => indentation,
        false => "",
    };
    let written = format!("{leading}{keyword} {condition}\n{indentation}{body_indent}{body}\n");
    written + &else_written(context, node, indentation, body_indent)
}

fn else_written(
    context: &RuleContext<'_>,
    node: Node<'_>,
    indentation: &str,
    body_indent: &str,
) -> String {
    let Some(alternative) = node.field("alternative") else {
        return "end".to_owned();
    };
    if alternative.kind_str() == "elsif" {
        return if_then_written(context, alternative, indentation, body_indent);
    }
    let source = statements_source(context, alternative).unwrap_or_default();
    format!("{indentation}else\n{indentation}{body_indent}{source}\n{indentation}end")
}

/// The source of everything a branch holds, which is one node upstream however many statements
/// were written.
fn statements_source(context: &RuleContext<'_>, branch: Node<'_>) -> Option<String> {
    let statements = super::nodes::children(branch);
    let first = statements.first()?;
    let last = statements.last()?;
    Some(
        context
            .source
            .slice(first.start_byte()..last.end_byte())
            .to_owned(),
    )
}
