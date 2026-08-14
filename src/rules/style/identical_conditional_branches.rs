//! `Style/IdenticalConditionalBranches`: what every branch does anyway belongs outside.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// `VARIABLES`: the node kinds upstream reads a variable off of rather than a call.
const VARIABLE_KINDS: &[&str] = &[
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
];

/// Where a hoisted expression goes.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Position {
    BeforeCondition,
    AfterCondition,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    // `add_offense` reports one range once: the head of a one-statement branch is also its tail,
    // and the second pass over it contributes neither an offense nor a correction.
    let mut reported: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["if", "unless"]) {
        let mut branches = vec![branch(node.field("consequence"))];
        expand_elses(node.field("alternative"), &mut branches);
        check_branches(context, &locals, node, &branches, &mut reported, offenses);
    }
    // A ternary is an `if` upstream, so it is reported -- though never corrected, since it has no
    // lines to move an expression between.
    for node in context.nodes_of("conditional") {
        let branches = vec![
            node.field("consequence").map(|only| vec![only]),
            node.field("alternative").map(|only| vec![only]),
        ];
        check_branches(context, &locals, node, &branches, &mut reported, offenses);
    }
    for node in context.nodes_of_any(&["case", "case_match"]) {
        let children = super::nodes::children(node);
        let Some(otherwise) = children.iter().find(|child| child.kind_str() == "else") else {
            continue;
        };
        let mut branches: Vec<Option<Vec<Node<'_>>>> = children
            .iter()
            .filter(|child| matches!(child.kind_str(), "when" | "in_clause"))
            .map(|clause| branch(clause.field("body")))
            .collect();
        branches.push(branch(Some(*otherwise)));
        check_branches(context, &locals, node, &branches, &mut reported, offenses);
    }
}

/// The statements a branch holds, or `None` for a branch that holds nothing -- which is what an
/// `if` without an `else` has, and what a branch of only comments comes out as.
fn branch<'t>(node: Option<Node<'t>>) -> Option<Vec<Node<'t>>> {
    let statements = super::nodes::children(node?);
    (!statements.is_empty()).then_some(statements)
}

/// `expand_elses`: an `elsif` is a nested conditional whose branches belong to this one.
fn expand_elses<'t>(alternative: Option<Node<'t>>, branches: &mut Vec<Option<Vec<Node<'t>>>>) {
    let Some(alternative) = alternative else {
        branches.push(None);
        return;
    };
    if alternative.kind_str() == "elsif" {
        branches.push(branch(alternative.field("consequence")));
        expand_elses(alternative.field("alternative"), branches);
        return;
    }
    branches.push(branch(Some(alternative)));
}

fn check_branches(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    branches: &[Option<Vec<Node<'_>>>],
    reported: &mut Vec<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    // An empty branch has nothing to move out, and nothing to compare the others against.
    let Some(branches): Option<Vec<&Vec<Node<'_>>>> = branches.iter().map(Option::as_ref).collect()
    else {
        return;
    };
    let tails: Vec<Node<'_>> = branches
        .iter()
        .filter_map(|branch| branch.last().copied())
        .collect();
    if duplicated(context, locals, node, &tails) {
        check_expressions(context, node, &tails, Position::AfterCondition, reported, offenses);
    }
    // The head of a branch holding one statement is also its tail: moving it out would leave the
    // branch with nothing to return.
    if last_child_of_parent(node) && branches.iter().any(|branch| branch.len() == 1) {
        return;
    }
    let heads: Vec<Node<'_>> = branches
        .iter()
        .filter_map(|branch| branch.first().copied())
        .collect();
    if !duplicated(context, locals, node, &heads) {
        return;
    }
    // Hoisting an assignment above the condition that reads what it assigns would change what the
    // condition sees.
    if let Some(head) = heads.first() {
        if let Some(assigned) = assigned_name(context, *head) {
            if condition_value(context, node).as_deref() == Some(assigned) {
                return;
            }
        }
    }
    check_expressions(context, node, &heads, Position::BeforeCondition, reported, offenses);
}

/// `duplicated_expressions?`: every branch ends -- or begins -- with the same expression.
fn duplicated(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    expressions: &[Node<'_>],
) -> bool {
    let Some(first) = expressions.first() else {
        return false;
    };
    if !expressions
        .iter()
        .all(|expression| super::nodes::same_tree(context, *first, *expression))
    {
        return false;
    }
    let Some(value) = assigned_value(*first) else {
        return true;
    };
    // An assignment of something the condition reads cannot move above the condition.
    let assigned = context.source.node_text(value);
    condition(node).is_none_or(|condition| {
        !super::nodes::children(condition).iter().any(|child| {
            is_variable(locals, *child) && context.source.node_text(*child) == assigned
        })
    })
}

fn check_expressions(
    context: &RuleContext<'_>,
    node: Node<'_>,
    expressions: &[Node<'_>],
    position: Position,
    reported: &mut Vec<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    // A conditional written on one line has no lines to move an expression between.
    let correctable = !written_inline(context, node);
    let mut inserted = false;
    for expression in expressions {
        if reported.contains(&expression.byte_range()) {
            continue;
        }
        reported.push(expression.byte_range());
        let message = format!(
            "Move `{}` out of the conditional.",
            context.source.node_text(*expression)
        );
        let mut offense = context.offense(message, expression.byte_range());
        if correctable {
            let mut edits = vec![remove(whole_lines(context, expression.byte_range()))];
            let mut anchor = node.byte_range();
            if !inserted {
                inserted = true;
                anchor = hoist(context, node, *expression, position, &mut edits);
            }
            offense = offense
                .corrected_by_all(edits)
                .corrections_anchored_at(anchor);
        }
        offenses.push(offense);
    }
}

/// `correct_assignment` / `correct_no_assignment`: where the expression is written instead, and
/// the range the insertion hangs off.
fn hoist(
    context: &RuleContext<'_>,
    node: Node<'_>,
    expression: Node<'_>,
    position: Position,
    edits: &mut Vec<Edit>,
) -> Range<usize> {
    let source = context.source.node_text(expression);
    let assignment = node.parent_of(context).filter(|parent| is_assignment(*parent));
    let anchored = assignment.unwrap_or(node);
    let indentation = " ".repeat(context.source.line_column(anchored.start_byte()).1 - 1);
    match (assignment, position) {
        // The assignment travels with the expression, leaving the conditional a statement.
        (Some(assignment), Position::AfterCondition) => {
            let prefix = context
                .source
                .slice(assignment.start_byte()..node.start_byte())
                .to_owned();
            edits.push(remove(assignment.start_byte()..node.start_byte()));
            edits.push(insert(
                node.end_byte(),
                format!("\n{indentation}{prefix}{source}"),
            ));
            node.byte_range()
        }
        (Some(assignment), Position::BeforeCondition) => {
            edits.push(insert(
                assignment.start_byte(),
                format!("{source}\n{indentation}"),
            ));
            assignment.byte_range()
        }
        (None, Position::AfterCondition) => {
            edits.push(insert(node.end_byte(), format!("\n{indentation}{source}")));
            node.byte_range()
        }
        (None, Position::BeforeCondition) => {
            edits.push(insert(
                node.start_byte(),
                format!("{source}\n{indentation}"),
            ));
            node.byte_range()
        }
    }
}

/// `node.ternary? || node.then?`: a conditional whose branches share a line with it.
fn written_inline(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() == "conditional" {
        return true;
    }
    if !matches!(node.kind_str(), "if" | "unless") {
        return false;
    }
    node.field("consequence")
        .and_then(|consequence| consequence.child(0))
        .is_some_and(|first| !first.is_named() && context.source.node_text(first) == "then")
}

/// `last_child_of_parent?`.
fn last_child_of_parent(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    if super::nodes::children(parent)
        .last()
        .is_none_or(|last| last.id() != node.id())
    {
        return false;
    }
    // A branch is not a node of its own upstream: the conditional holds its branches directly, so
    // what follows the branch this one closes is the branch after it.
    match parent.kind_str() {
        "then" => parent
            .parent()
            .is_none_or(|conditional| conditional.field("alternative").is_none()),
        _ => true,
    }
}

/// `node.condition`, which a `case` names its subject with.
fn condition<'t>(node: Node<'t>) -> Option<Node<'t>> {
    node.field("condition")
        .or_else(|| node.field("value"))
}

/// `assignable_condition_value`: the name the condition tests, when it tests one.
fn condition_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let condition = condition(node)?;
    match condition.kind_str() {
        "call" => Some(
            condition
                .field("receiver")
                .map_or_else(
                    || context.source.node_text(condition),
                    |receiver| context.source.node_text(receiver),
                )
                .to_owned(),
        ),
        "binary" => condition
            .field("left")
            .map(|left| context.source.node_text(left).to_owned()),
        kind if VARIABLE_KINDS.contains(&kind) => {
            Some(context.source.node_text(condition).to_owned())
        }
        _ => None,
    }
}

/// `node_parts[0].to_s` of an assignment: the name it binds, for the spellings that name one.
fn assigned_name<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    if node.kind_str() != "assignment" || !is_assignment(node) {
        return None;
    }
    let left = node.field("left")?;
    // A multiple assignment and a shorthand one both stand for a node rather than a name upstream,
    // which never equals what the condition spells.
    (VARIABLE_KINDS.contains(&left.kind_str()) || left.kind_str() == "constant")
        .then(|| context.source.node_text(left))
}

/// `unique_expression.child_nodes.first` of an assignment: the value for a single assignment, the
/// left-hand side for the spellings whose name upstream keeps as a symbol.
fn assigned_value<'t>(node: Node<'t>) -> Option<Node<'t>> {
    if !is_assignment(node) {
        return None;
    }
    let left = node.field("left")?;
    match node.kind_str() == "assignment" && !matches!(left.kind_str(), "left_assignment_list") {
        true => node.field("right"),
        false => Some(left),
    }
}

/// `assignment?`: `a.b = 1` and `a[0] = 1` are calls upstream, not assignments.
fn is_assignment(node: Node<'_>) -> bool {
    match node.kind_str() {
        "assignment" => node
            .field("left")
            .is_some_and(|left| !matches!(left.kind_str(), "call" | "element_reference")),
        "operator_assignment" => true,
        _ => false,
    }
}

/// `range_by_whole_lines(range, include_final_newline: true)`.
fn whole_lines(context: &RuleContext<'_>, range: Range<usize>) -> Range<usize> {
    let first = context.source.line_column(range.start).0;
    let last = context.source.line_column(range.end).0;
    context.source.line_start(first)..context.source.line_range(last).end
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

fn insert(at: usize, text: String) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text,
        safe: true,
    }
}

/// `variable?`: a bare name is one only where upstream's parser built an `lvar` for it.
fn is_variable(locals: &LocalVariables<'_, '_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "identifier" => locals.is_lvar(node),
        kind => VARIABLE_KINDS.contains(&kind),
    }
}
