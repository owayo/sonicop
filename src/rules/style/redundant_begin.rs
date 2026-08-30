use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node;

use super::conditional::{UpstreamParent, body_of, upstream_parent};
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

const MSG: &str = "Redundant `begin` block detected.";

/// The version that let a `do ... end` block carry a `rescue` without a `begin` of its own.
const IMPLICIT_BEGIN_VERSION: RubyVersion = RubyVersion::new(2, 5);

/// The clauses that make the parser put a `rescue` or an `ensure` node between the `kwbegin` and
/// its statements, which is what leaves it a single child.
/// `contain_rescue_or_ensure?` asks for a `rescue` or an `ensure` and **not** for an `else`: a
/// `begin ... else ... end` written without a `rescue` is the redundant `begin` this cop reports.
const CLAUSES: &[&str] = &["rescue", "ensure"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `add_offense` refuses a range it has already reported, and every handler here reports the
    // `begin` keyword of the block it found, so the same block reached twice costs one offense.
    let mut reported: HashSet<usize> = HashSet::new();

    // `on_def` / `on_defs`.
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        if is_endless(node) {
            continue;
        }
        if let Some(body) = definition_body(node).filter(|body| body.kind_str() == "begin") {
            register_offense(context, offenses, &mut reported, body);
        }
    }

    // `on_if` / `on_case` / `on_case_match`: a branch written as a `begin ... end` that handles
    // nothing itself.
    for node in
        context.nodes_of_any(&["if", "elsif", "unless", "conditional", "case", "case_match"])
    {
        for branch in branches(node) {
            if branch.kind_str() != "begin" || has_clause(branch) {
                continue;
            }
            register_offense(context, offenses, &mut reported, branch);
        }
    }

    // `on_while` / `on_until`.
    for node in context.nodes_of_any(&["while", "until"]) {
        let Some(body) = node
            .field("body")
            .and_then(|body| body_of(body).single())
        else {
            continue;
        };
        if body.kind_str() != "begin" || has_clause(body) {
            continue;
        }
        register_offense(context, offenses, &mut reported, body);
    }

    // `on_block` / `on_numblock` / `on_itblock`: only a `do ... end` block, which is the only one
    // that can carry the `rescue` itself.
    if context.target_ruby_version() >= IMPLICIT_BEGIN_VERSION {
        for node in context.nodes_of("call") {
            let Some(block) = node
                .field("block")
                .filter(|block| block.kind_str() == "do_block")
            else {
                continue;
            };
            let Some(body) = block
                .field("body")
                .and_then(|body| body_of(body).single())
                .filter(|body| body.kind_str() == "begin")
            else {
                continue;
            };
            register_offense(context, offenses, &mut reported, body);
        }
    }

    // `on_kwbegin`: the *last* `begin ... end` in the subtree that no context excuses. A block
    // that stands around another one therefore reports the inner one, and its own turn comes on
    // the next pass, once the inner one is gone.
    let offensive: Vec<Node<'_>> = context
        .nodes_of("begin")
        .filter(|node| !allowable(context, *node))
        .collect();
    for node in context.nodes_of("begin") {
        let range = node.byte_range();
        let Some(target) = offensive
            .iter()
            .rev()
            .find(|candidate| range.contains(&candidate.start_byte()))
        else {
            continue;
        };
        register_offense(context, offenses, &mut reported, *target);
    }
}

/// `allowable_kwbegin?`: the contexts in which a `begin ... end` is doing something.
fn allowable(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let children = child_count(node);
    // `empty_begin?`.
    if children == 0 {
        return true;
    }
    let parent = upstream_parent(node);
    // `begin_block_has_multiline_statements?`.
    if parent.is_some() && children >= 2 {
        return true;
    }
    // `contain_rescue_or_ensure?`.
    if has_clause(node) {
        return true;
    }
    // `valid_context_using_only_begin?`.
    let Some(UpstreamParent::Node(parent)) = parent else {
        return false;
    };
    (is_assignment(parent) && children != 1)
        || is_post_condition_loop(parent, node)
        || is_send(context, parent)
        || is_operator_keyword(context, parent)
}

fn register_offense(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut HashSet<usize>,
    node: Node<'_>,
) {
    let (Some(begin_keyword), Some(end_keyword)) = (keyword(node, "begin"), keyword(node, "end"))
    else {
        return;
    };
    let range = begin_keyword.byte_range();
    if !reported.insert(range.start) {
        return;
    }

    let parent = match upstream_parent(node) {
        Some(UpstreamParent::Node(parent)) => Some(parent),
        _ => None,
    };
    let mut edits = Vec::new();
    let mut anchor: Option<Range<usize>> = None;

    match parent.filter(|parent| is_assignment(*parent)) {
        Some(parent) => {
            replace_begin_with_statement(context, node, &range, parent, &mut edits, &mut anchor);
        }
        None => {
            // `remove_begin`: an endless definition has no line of its own for the keyword to
            // vacate, so the space around it goes too.
            let removal = match parent.filter(|parent| is_endless(*parent)) {
                Some(_) => with_surrounding_space(context, &range),
                None => range.clone(),
            };
            edits.push(removal_of(removal));
        }
    }

    // `use_modifier_form_after_multiline_begin_block?`.
    if let Some(parent) = parent.filter(|parent| is_modifier_conditional(*parent, node)) {
        if node.start_position().row != node.end_position().row {
            correct_modifier_form(context, node, parent, &mut edits, &mut anchor);
        }
    }

    edits.push(removal_of(end_keyword.byte_range()));

    let mut offense = context.offense(MSG, range);
    if let Some(anchor) = anchor {
        offense = offense.corrections_anchored_at(anchor);
    }
    offenses.push(offense.corrected_by_all(edits));
}

/// `replace_begin_with_statement`: an assignment keeps its right-hand side, so the keyword is
/// overwritten with the statement it wrapped rather than deleted.
fn replace_begin_with_statement(
    context: &RuleContext<'_>,
    node: Node<'_>,
    range: &Range<usize>,
    parent: Node<'_>,
    edits: &mut Vec<Edit>,
    anchor: &mut Option<Range<usize>>,
) {
    let Some(first) = super::nodes::children_in(node, context).first().copied() else {
        return;
    };
    let source = context.source.node_text(first);
    let source = match is_modifier_conditional_form(first) {
        true => format!("({source})"),
        false => source.to_owned(),
    };
    edits.push(Edit {
        start: range.start,
        end: range.end,
        replacement: source,
        safe: true,
    });
    edits.push(removal_of(range.end..first.end_byte()));

    // `restore_removed_comments`: a comment written between the keyword and the statement would
    // otherwise be deleted with the text around it, so it moves above the assignment.
    let comments = context.source.slice(range.end..first.start_byte());
    if !comments.trim().is_empty() {
        let comments = comments.to_owned();
        edits.push(Edit {
            start: parent.start_byte(),
            end: parent.start_byte(),
            replacement: comments,
            safe: true,
        });
        // `insert_before(node.parent, ...)` hands the corrector the assignment rather than the
        // keyword this offense was reported on.
        *anchor = Some(parent.byte_range());
    }
}

/// `correct_modifier_form_after_multiline_begin_block`: `begin ... end if cond` becomes a modifier
/// on the statement itself, so the condition moves up and its line goes.
fn correct_modifier_form(
    context: &RuleContext<'_>,
    node: Node<'_>,
    parent: Node<'_>,
    edits: &mut Vec<Edit>,
    anchor: &mut Option<Range<usize>>,
) {
    let (Some(first), Some(keyword), Some(condition)) = (
        super::nodes::children_in(node, context).first().copied(),
        modifier_keyword(parent),
        parent.field("condition"),
    ) else {
        return;
    };
    let condition = keyword.start_byte()..condition.end_byte();
    edits.push(Edit {
        start: first.end_byte(),
        end: first.end_byte(),
        replacement: format!(" {}", context.source.slice(condition.clone())),
        safe: true,
    });
    // `insert_after(node.children.first, ...)` hands the corrector the statement's own range.
    *anchor = Some(first.byte_range());
    edits.push(removal_of(whole_lines(context, &condition)));
}

fn removal_of(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// `range_with_surrounding_space(range, newlines: true)`: spaces and tabs on either side, then the
/// newlines beyond them.
fn with_surrounding_space(context: &RuleContext<'_>, range: &Range<usize>) -> Range<usize> {
    support::range_with_surrounding_space(
        range.clone(),
        context.source.text(),
        support::Side::Both,
        false,
        true,
        false,
    )
}

/// `range_by_whole_lines(range, include_final_newline: true)`.
fn whole_lines(context: &RuleContext<'_>, range: &Range<usize>) -> Range<usize> {
    let text = context.source.text().as_bytes();
    let mut start = range.start;
    while start > 0 && text[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = range.end;
    while end < text.len() && text[end] != b'\n' {
        end += 1;
    }
    start..(end + 1).min(text.len())
}

/// The statements the `begin ... end` was written with, in the shape the parser hands them out: a
/// `rescue` or an `ensure` clause leaves the `kwbegin` a single child however much it holds.
fn child_count(node: Node<'_>) -> usize {
    match has_clause(node) {
        true => 1,
        false => super::nodes::children(node).len(),
    }
}

fn has_clause(node: Node<'_>) -> bool {
    super::nodes::children(node)
        .iter()
        .any(|child| CLAUSES.contains(&child.kind_str()))
}

/// `node.body` of a definition: the statement it holds, or nothing when it holds several or was
/// split by a `rescue`.
fn definition_body<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    body_of(node.field("body")?).single()
}

/// `DefNode#endless?`: a definition written with `=` has no `end` to close it.
fn is_endless(node: Node<'_>) -> bool {
    if !matches!(node.kind_str(), "method" | "singleton_method") {
        return false;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind_str() == "=")
}

/// `IfNode#branches` / `CaseNode#branches` / `CaseMatchNode#branches`, as the bodies this cop
/// inspects.
///
/// An `elsif` is a nested `if` upstream and gets its own turn, so a branch that is one is left to
/// it rather than flattened in here. An `unless` is an `if` with its branches swapped there, and
/// `IfNode#node_parts` swaps them back, so both read the same way round.
fn branches<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let mut push = |body: Option<Node<'tree>>| {
        if let Some(body) = body.and_then(|body| body_of(body).single()) {
            found.push(body);
        }
    };
    match node.kind_str() {
        "case" | "case_match" => {
            for clause in super::nodes::children(node) {
                match clause.kind_str() {
                    "when" | "in_clause" => push(clause.field("body")),
                    "else" => push(Some(clause)),
                    _ => {}
                }
            }
        }
        // A ternary names its branches rather than wrapping them in `then` and `else`.
        "conditional" => {
            for field in ["consequence", "alternative"] {
                if let Some(branch) = node.field(field) {
                    found.push(branch);
                }
            }
        }
        "if" | "elsif" | "unless" => {
            push(node.field("consequence"));
            push(
                node.field("alternative")
                    .filter(|alternative| alternative.kind_str() == "else"),
            );
        }
        _ => {}
    }
    found
}

/// `Node#assignment?`, which `SendNode` widens to any call to a setter: `a.b = v` and
/// `a[i] = v` answer it as truly as `a = v` does.
fn is_assignment(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "assignment" | "operator_assignment")
}

/// Whether a plain `=` writes through a method rather than to a name, which is what makes it a
/// `send` -- `a.b = v` is `(send a :b= v)` upstream and `a[i] = v` is `(send a :[]= i v)`.
fn is_attribute_assignment(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"))
}

/// `parent&.post_condition_loop?`: `begin ... end while cond`, which is a `while_post` upstream
/// only because its body is the `kwbegin`.
fn is_post_condition_loop(parent: Node<'_>, node: Node<'_>) -> bool {
    matches!(parent.kind_str(), "while_modifier" | "until_modifier")
        && parent
            .field("body")
            .is_some_and(|body| body.id() == node.id())
}

/// `parent&.send_type?`. A safe navigation call is a `csend` upstream and does not count, and
/// `defined?` is a node of its own however the grammar spells it.
fn is_send(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "call" => send_node::is_plain_send(node, context),
        "element_reference" => true,
        // `a.b = v` and `a[i] = v` are `send`s whose method ends in `=`.
        "assignment" => {
            is_attribute_assignment(node)
                && node
                    .field("left")
                    .is_some_and(|left| left.kind_str() != "call" || send_node::is_plain_send(left, context))
        }
        "unary" => node
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) != "defined?"),
        "binary" => !is_operator_keyword(context, node),
        _ => false,
    }
}

/// `parent&.operator_keyword?`: `and` and `or`, which `&&` and `||` build just the same.
fn is_operator_keyword(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| {
                matches!(
                    context.source.node_text(operator),
                    "and" | "or" | "&&" | "||"
                )
            })
}

/// Whether the node is an `if` or `unless` written after what it guards, with `node` as its body.
fn is_modifier_conditional(parent: Node<'_>, node: Node<'_>) -> bool {
    matches!(parent.kind_str(), "if_modifier" | "unless_modifier")
        && parent
            .field("body")
            .is_some_and(|body| body.id() == node.id())
}

fn is_modifier_conditional_form(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "if_modifier" | "unless_modifier")
}

fn modifier_keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && matches!(child.kind_str(), "if" | "unless"))
}

/// `loc.begin` / `loc.end` of the `begin ... end`.
fn keyword<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let mut found = None;
    for child in node.children(&mut cursor) {
        if !child.is_named() && child.kind_str() == kind {
            found = Some(child);
            if kind == "begin" {
                break;
            }
        }
    }
    found
}
