//! `Layout/EmptyLineAfterGuardClause`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MESSAGE: &str = "Add empty line after guard clause.";

const CONDITIONALS: [&str; 5] = [
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // The grammar files a heredoc body next to the statement its opener was written in rather
    // than inside it, so the two are matched up by the order they appear in.
    let beginnings: Vec<_> = context.nodes_of("heredoc_beginning").collect();
    let bodies: Vec<_> = context.nodes_of("heredoc_body").collect();
    let heredocs = Heredocs {
        beginnings: &beginnings,
        bodies: &bodies,
    };
    for node in context.nodes_of_any(&CONDITIONALS) {
        inspect(context, heredocs, node, offenses);
    }
}

fn inspect<'tree>(
    context: &RuleContext<'tree>,
    heredocs: Heredocs<'_, 'tree>,
    node: Node<'tree>,
    offenses: &mut Vec<Offense>,
) {
    let text = context.source.text();
    let Some(branch) = if_branch(node) else {
        return;
    };
    if !is_guard_clause(text, branch) {
        return;
    }
    // Every one of the remaining `correct_style?` clauses comes down to the guard clause having
    // no statement after it: without a right sibling upstream stops, and a sibling only exists
    // inside a statement list.
    let Some((next, begin_parent)) = next_statement(node) else {
        return;
    };
    if CONDITIONALS.contains(&next.kind_str())
        && if_branch(next).is_some_and(|branch| is_guard_clause_branch(text, branch))
    {
        return;
    }
    if begin_parent && next.start_position().row == node.start_position().row {
        return;
    }

    let modifier_form = matches!(node.kind_str(), "if_modifier" | "unless_modifier");
    let heredoc = modifier_form
        .then(|| last_heredoc_argument(context, node, true))
        .flatten();
    let last_line = node.end_position().row + 1;

    if let Some(heredoc) = heredoc {
        let body = heredocs.body_of(heredoc);
        let Some(body) = body else { return };
        let lines = heredoc_lines(context, body);
        if next_line_clear(context, last_line + lines) {
            return;
        }
        let terminator = terminator_range(context, body);
        offenses.push(
            context
                .offense(MESSAGE, terminator.clone())
                .corrected_by(insertion(context, body.end_byte())),
        );
        return;
    }

    if next_line_clear(context, last_line) {
        return;
    }
    let range = end_keyword(node).map_or_else(|| node.byte_range(), |keyword| keyword.byte_range());
    offenses.push(
        context
            .offense(MESSAGE, range)
            .corrected_by(insertion(context, node.end_byte())),
    );
}

/// `corrector.insert_after(range_by_whole_lines(...), "\n")`, stepping over a directive comment on
/// the line that follows.
fn insertion(context: &RuleContext<'_>, end: usize) -> Edit {
    let line = context.source.line_column(end).0;
    let mut offset = line_end(context, line);
    if let Some(comment) = allowed_directive_comment(context, line + 1) {
        offset = comment.end;
    }
    Edit {
        start: offset,
        end: offset,
        replacement: "\n".to_owned(),
        safe: true,
    }
}

fn line_end(context: &RuleContext<'_>, line: usize) -> usize {
    let range = context.source.line_range(line);
    range.start + context.source.line(line).trim_end_matches('\n').len()
}

/// `next_line_empty_or_allowed_directive_comment?`.
fn next_line_clear(context: &RuleContext<'_>, line: usize) -> bool {
    if context.source.line(line + 1).trim().is_empty() {
        return true;
    }
    allowed_directive_comment(context, line + 1).is_some()
        && context.source.line(line + 2).trim().is_empty()
}

/// A `# rubocop:enable ...` or SimpleCov marker occupying the whole of `line`.
fn allowed_directive_comment(context: &RuleContext<'_>, line: usize) -> Option<Range<usize>> {
    let text = context.source.text();
    let comment = context
        .comment_ranges()
        .iter()
        .find(|range| context.source.line_column(range.start).0 == line)?;
    let body = &text[comment.clone()];
    (is_enable_directive(body) || is_simplecov_directive(body)).then(|| comment.clone())
}

/// `DirectiveComment#enabled?`: the comment turns cops back on.
fn is_enable_directive(comment: &str) -> bool {
    let Some(rest) = comment.strip_prefix('#') else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix("rubocop") else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix("enable") else {
        return false;
    };
    // `\b` after the mode, then the cop list the directive is required to carry.
    !rest.starts_with(|character: char| character.is_alphanumeric() || character == '_')
        && !rest.trim().is_empty()
}

fn is_simplecov_directive(comment: &str) -> bool {
    let Some(rest) = comment.strip_prefix('#') else {
        return false;
    };
    let rest = rest.trim_start();
    if rest.starts_with(":nocov:") {
        return true;
    }
    let Some(rest) = rest.strip_prefix("simplecov") else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    let rest = rest.trim_start();
    for mode in ["disable", "enable"] {
        if let Some(tail) = rest.strip_prefix(mode) {
            if !tail.starts_with(|character: char| character.is_alphanumeric() || character == '_')
            {
                return true;
            }
        }
    }
    false
}

/// `IfNode#if_branch`, already normalized for `unless`.
fn if_branch<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "if_modifier" | "unless_modifier" => node.field("body"),
        _ => {
            let branch = node.field("consequence")?;
            if branch.kind_str() != "then" {
                return Some(branch);
            }
            // A `then` clause holding more than one statement is a `begin` upstream, which is
            // never a guard clause.
            let mut cursor = branch.walk();
            let mut statements = branch
                .named_children(&mut cursor)
                .filter(|child| !matches!(child.kind_str(), "heredoc_body" | "comment"));
            let first = statements.next()?;
            statements.next().is_none().then_some(first)
        }
    }
}

/// `Node#guard_clause?`.
fn is_guard_clause(text: &str, branch: Node<'_>) -> bool {
    let node = operator_keyword_rhs(text, branch).unwrap_or(branch);
    node.start_position().row == node.end_position().row && is_guard_kind(text, node)
}

/// `guard_clause_branch?`, which drops the single-line requirement.
fn is_guard_clause_branch(text: &str, branch: Node<'_>) -> bool {
    is_guard_clause(text, branch) || is_guard_kind(text, branch)
}

fn is_guard_kind(text: &str, node: Node<'_>) -> bool {
    match node.kind_str() {
        "return" | "break" | "next" => true,
        "identifier" => matches!(&text[node.byte_range()], "raise" | "fail"),
        "call" => {
            node.field("receiver").is_none()
                && node
                    .field("method")
                    .is_some_and(|method| matches!(&text[method.byte_range()], "raise" | "fail"))
        }
        _ => false,
    }
}

fn operator_keyword_rhs<'tree>(text: &str, node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind_str() != "binary" {
        return None;
    }
    let operator = node.field("operator")?;
    matches!(&text[operator.byte_range()], "&&" | "||" | "and" | "or")
        .then(|| node.field("right"))
        .flatten()
}

fn end_keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == "end")
}

/// The statement written after `node` in the same list, which is what upstream reaches through
/// `right_sibling`.
fn next_statement<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, bool)> {
    let parent = node.parent()?;
    // A `begin ... end` block holds its statements directly, so its children have a `kwbegin`
    // parent that `begin_type?` says no to. Every other list becomes a `begin` past one statement,
    // and a parenthesized one becomes a `begin` even with a single statement in it.
    let begin_parent = match parent.kind_str() {
        "begin" => false,
        "parenthesized_statements" => true,
        "body_statement" | "block_body" | "program" | "then" | "else" | "do" | "ensure" => true,
        _ => return None,
    };
    let mut cursor = parent.walk();
    let statements: Vec<Node<'tree>> = parent
        .named_children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind_str(),
                "rescue" | "ensure" | "else" | "heredoc_body" | "comment"
            )
        })
        .collect();
    let index = statements.iter().position(|child| *child == node)?;
    statements
        .get(index + 1)
        .map(|next| (*next, begin_parent && statements.len() > 1))
}

/// Where the `<<~FOO` openers of the file sit, paired with the bodies that follow them.
#[derive(Clone, Copy)]
struct Heredocs<'a, 'tree> {
    beginnings: &'a [Node<'tree>],
    bodies: &'a [Node<'tree>],
}

impl<'tree> Heredocs<'_, 'tree> {
    fn body_of(&self, beginning: Node<'_>) -> Option<Node<'tree>> {
        let index = self
            .beginnings
            .iter()
            .position(|candidate| *candidate == beginning)?;
        self.bodies.get(index).copied()
    }
}

/// `heredoc_body.last_line - heredoc_body.first_line`, where upstream's body starts on the line
/// after the opener and ends on the terminator's.
fn heredoc_lines(context: &RuleContext<'_>, body: Node<'_>) -> usize {
    let first = context.source.line_column(body.start_byte()).0 + 1;
    let last = context.source.line_column(body.end_byte()).0;
    last.saturating_sub(first) + 1
}

/// `loc.heredoc_end`, which covers the terminator together with the indentation before it.
fn terminator_range(context: &RuleContext<'_>, body: Node<'_>) -> Range<usize> {
    let end = body.end_byte();
    let line = context.source.line_column(end).0;
    context.source.line_start(line)..end
}

/// `last_heredoc_argument`.
fn last_heredoc_argument<'tree>(
    context: &RuleContext<'tree>,
    node: Node<'tree>,
    conditional: bool,
) -> Option<Node<'tree>> {
    let text = context.source.text();
    let mut current = if conditional {
        let branch = if_branch(node)?;
        if is_and(text, branch) {
            branch.field("left")?
        } else if let Some(condition) = node
            .field("condition")
            .filter(|condition| holds_heredoc(*condition))
        {
            condition
        } else {
            last_child(branch)?
        }
    } else {
        node
    };
    while matches!(current.kind_str(), "begin" | "parenthesized_statements") {
        current = current.named_child(0)?;
    }
    if current.kind_str() == "heredoc_beginning" {
        return Some(current);
    }
    for argument in call_arguments(current) {
        if let Some(found) = last_heredoc_argument(context, argument, false) {
            return Some(found);
        }
    }
    let receiver = current.field("receiver")?;
    last_heredoc_argument(context, receiver, false)
}

/// `node.children.last`, which for a call or a `return` is its final argument. Anything else is a
/// bare symbol upstream, which the search stops at.
fn last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    call_arguments(node).last().copied()
}

fn is_and(text: &str, node: Node<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(&text[operator.byte_range()], "&&" | "and"))
}

fn call_arguments<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    let Some(list) = node
        .children(&mut cursor)
        .find(|child| child.kind_str() == "argument_list")
    else {
        return Vec::new();
    };
    let mut inner = list.walk();
    list.named_children(&mut inner).collect()
}

fn holds_heredoc(node: Node<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "heredoc_beginning" {
            return true;
        }
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
    }
    false
}
