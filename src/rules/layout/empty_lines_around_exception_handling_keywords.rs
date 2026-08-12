//! `Layout/EmptyLinesAroundExceptionHandlingKeywords`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_def`, `on_defs`, `on_block`, `on_numblock` and `on_kwbegin`. A `class` or `module` body
    // may carry the same clauses, and the cop does not look at one.
    for node in context.nodes_of_any(&["method", "singleton_method", "block", "do_block", "begin"])
    {
        let clauses = body_clauses(node);
        let rescues: Vec<Node<'_>> = clauses
            .iter()
            .copied()
            .filter(|clause| clause.kind() == "rescue")
            .collect();
        let ensure = clauses.iter().copied().find(|c| c.kind() == "ensure");
        let else_clause = clauses.iter().copied().find(|c| c.kind() == "else");
        // `keyword_locations` answers with nothing unless the body is a `rescue` or an `ensure`.
        if rescues.is_empty() && ensure.is_none() {
            continue;
        }
        let Some(end) = closing_keyword(node) else {
            continue;
        };
        if last_body_and_end_on_same_line(context, ensure, else_clause, &rescues, end) {
            continue;
        }
        let owner_line = context
            .source
            .line_column(super::support::parser_node_start(node))
            .0;

        for clause in ensure.into_iter().chain(else_clause).chain(rescues) {
            let Some(keyword) = clause.child(0) else {
                continue;
            };
            let line = context.source.line_column(keyword.start_byte()).0;
            if line == owner_line {
                continue;
            }
            let keyword = &context.source.text()[keyword.byte_range()];
            report(context, offenses, line + 1, "after", keyword);
            report(context, offenses, line - 1, "before", keyword);
        }
    }
}

/// `check_line`: an empty line next to the keyword is one line too many, and the offense points at
/// the line break that line is made of.
fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    line: usize,
    location: &str,
    keyword: &str,
) {
    if line == 0 || line > context.source.line_count() {
        return;
    }
    // `String#empty?`: a line holding blanks is not an empty line.
    if !context
        .source
        .line(line)
        .trim_end_matches(['\n', '\r'])
        .is_empty()
    {
        return;
    }
    let start = context.source.line_start(line);
    let range = start..(start + 1).min(context.source.text().len());
    offenses.push(
        context
            .offense(
                format!("Extra empty line detected {location} the `{keyword}`."),
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

/// The `rescue`, `else` and `ensure` clauses of a body. A `begin ... end` carries them directly,
/// while everything else keeps its statements in a body node.
fn body_clauses<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let container = match node.kind() {
        "begin" => node,
        _ => match node.child_by_field_name("body") {
            Some(body) if body.kind() == "body_statement" => body,
            _ => return Vec::new(),
        },
    };
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .filter(|child| matches!(child.kind(), "rescue" | "else" | "ensure"))
        .collect()
}

/// `last_body_and_end_on_same_line?`: with the whole construct written on one line there is no
/// blank line to be found around anything.
fn last_body_and_end_on_same_line(
    context: &RuleContext<'_>,
    ensure: Option<Node<'_>>,
    else_clause: Option<Node<'_>>,
    rescues: &[Node<'_>],
    end: Node<'_>,
) -> bool {
    let end_line = context.source.line_column(end.start_byte()).0;
    // The `ensure` node upstream reaches from the protected body to the end of the ensure body, so
    // its last line is where that body ends.
    if let Some(ensure) = ensure {
        return context.source.line_column(ensure.end_byte()).0 == end_line;
    }
    let last = else_clause.or_else(|| rescues.last().copied());
    last.is_some_and(|node| context.source.line_column(node.start_byte()).0 == end_line)
}

/// `node.loc.end`: the `end` of a definition or a `begin`, or the `}` a brace block closes with.
fn closing_keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let last = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
    matches!(last.kind(), "end" | "}").then_some(last)
}
