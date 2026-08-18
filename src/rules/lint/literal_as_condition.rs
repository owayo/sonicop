use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;
use crate::rules::send_node::named_children;

use super::literals::{is_basic_literal, is_falsey_literal, is_literal, is_truthy_literal};
use super::statements::{Branch, statements};
use crate::rules::node_ext::NodeExt;

/// The nodes whose `condition` the parser rewrites: a range there becomes a flip-flop and a regexp
/// a match against `$_`, and neither is a literal any more.
const CONDITION_OWNERS: &[&str] = &[
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
];

/// `Builder#check_condition`, which reaches through the parentheses and the `and`/`or` operators
/// a condition may be written with.
fn rewritten_in_condition(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !matches!(node.kind_str(), "range" | "regex") {
        return false;
    }
    let mut current = node;
    loop {
        let Some(parent) = current.parent_of(context) else {
            return false;
        };
        let reaches_through = match parent.kind_str() {
            "parenthesized_statements" => statements(parent).len() == 1,
            "binary" => parent
                .field("operator")
                .is_some_and(|operator| {
                    matches!(
                        context.source.node_text(operator),
                        "&&" | "and" | "||" | "or"
                    )
                }),
            _ => false,
        };
        if reaches_through {
            current = parent;
            continue;
        }
        // `Builder#not_op` runs its operand through `check_condition` too, wherever it is written.
        if parent.kind_str() == "unary"
            && parent
                .field("operator")
                .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
        {
            return parent
                .field("operand")
                .is_some_and(|operand| operand.id() == current.id());
        }
        return CONDITION_OWNERS.contains(&parent.kind_str())
            && parent
                .field("condition")
                .is_some_and(|condition| condition.id() == current.id());
    }
}

fn truthy(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    !rewritten_in_condition(node, context) && is_truthy_literal(node, context)
}

fn falsey(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    !rewritten_in_condition(node, context) && is_falsey_literal(node, context)
}

fn literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    !rewritten_in_condition(node, context) && is_literal(node, context)
}

/// Every node kind the cop has a handler for, in the order the commissioner reaches them.
const HANDLED: &[&str] = &[
    "binary",
    "unary",
    // `x.!` is the same `(send _ :!)` upstream reaches through `on_send`, but the grammar writes
    // it as a `call`. Without it here the negation entry is never reached for that spelling, and
    // `if 1.!` / `while 1.!` / `until 1.!` / `1.! ? a : b` / `s if 1.!` all go unreported.
    "call",
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
    "case",
    "case_match",
];

/// The keywords whose value is void, which cannot be moved to the left of an `&&`.
const VOID_KEYWORDS: &[&str] = &["return", "break", "next"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `ignore_node`: once an `if` has been rewritten whole, the conditionals inside it are
    // reported without a correction of their own.
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(HANDLED) {
        match node.kind_str() {
            "binary" => check_operator_keyword(node, context, offenses),
            "unary" | "call" => check_negation(node, context, offenses),
            "while" | "while_modifier" => check_loop(node, true, context, offenses),
            "until" | "until_modifier" => check_loop(node, false, context, offenses),
            "case" => check_case(node, context, offenses),
            "case_match" => check_case_match(node, context, offenses),
            _ => check_if(node, context, offenses, &mut ignored),
        }
    }
}

fn message(literal: &str) -> String {
    format!("Literal `{literal}` appeared as a condition.")
}

fn report(range: Range<usize>, context: &RuleContext<'_>) -> Offense {
    context.offense(message(context.source.slice(range.clone())), range)
}

/// `on_and` and `on_or`: only the left operand decides the result of the operator.
fn check_operator_keyword(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(operator) = node.field("operator") else {
        return;
    };
    let conjunction = match context.source.node_text(operator) {
        "&&" | "and" => true,
        "||" | "or" => false,
        _ => return,
    };
    let (Some(left), Some(right)) = (
        node.field("left"),
        node.field("right"),
    ) else {
        return;
    };
    let decisive = if conjunction {
        truthy(left, context)
    } else {
        falsey(left, context)
    };
    if !decisive {
        return;
    }
    let offense = report(left.byte_range(), context);
    // `'foo' && return` cannot become a bare `return` where a value is expected.
    offenses.push(if VOID_KEYWORDS.contains(&right.kind_str()) {
        offense
    } else {
        offense
            .corrections_anchored_at(node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: context.source.node_text(right).to_owned(),
                safe: true,
            })
    });
}

/// `on_send` with `RESTRICT_ON_SEND = [:!]`: what a negation is applied to is a condition too.
///
/// **The entry uses `negation_method?`, not `prefix_bang?`.** That is why `if not 1` is reported
/// here while `check_node` -- the recursive half -- leaves it alone: the two halves of this cop ask
/// different questions, and answering both with one predicate loses a form either way.
fn check_negation(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(found) = send_node::negation(node, context) else {
        return;
    };
    if literal(found.operand, context) {
        offenses.push(report(found.operand.byte_range(), context));
        return;
    }
    check_node(found.operand, context, offenses);
}

/// `check_node`: the shapes whose operands are conditions in their own right.
fn check_node(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // **`prefix_bang?` here, `negation_method?` at the entry.** `not` is excluded on this side, so
    // widening both to the same predicate reports an operand upstream never looks at.
    if let Some(found) = send_node::bang(node, context) {
        handle_node(found.operand, context, offenses);
        return;
    }
    match node.kind_str() {
        "binary"
            if node
                .field("operator")
                .is_some_and(|operator| {
                    matches!(
                        context.source.node_text(operator),
                        "&&" | "and" | "||" | "or"
                    )
                }) =>
        {
            for side in ["left", "right"] {
                if let Some(operand) = node.field(side) {
                    handle_node(operand, context, offenses);
                }
            }
        }
        // `(x)` is a `begin` upstream, and only a `begin` holding one expression is a condition.
        "parenthesized_statements" => {
            let inner = statements(node);
            if inner.len() == 1 {
                handle_node(inner[0], context, offenses);
            }
        }
        _ => {}
    }
}

/// `handle_node`.
fn handle_node(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if literal(node, context) {
        // The left operand of an `and` is already `on_and`'s to report.
        if node.parent_of(context).is_some_and(|parent| {
            parent.kind_str() == "binary"
                && parent
                    .field("operator")
                    .is_some_and(|operator| {
                        matches!(context.source.node_text(operator), "&&" | "and")
                    })
        }) {
            return;
        }
        offenses.push(report(node.byte_range(), context));
        return;
    }
    if matches!(
        node.kind_str(),
        "call" | "unary" | "binary" | "parenthesized_statements" | "element_reference"
    ) {
        check_node(node, context, offenses);
    }
}

/// `on_while` and `on_until`, which keep a literal loop running but say so in one word.
fn check_loop(
    node: Node<'_>,
    is_while: bool,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(condition) = node.field("condition") else {
        return;
    };
    let keep = if is_while { "true" } else { "false" };
    if context.source.node_text(condition) == keep {
        return;
    }
    let truthy = truthy(condition, context);
    let falsey = falsey(condition, context);
    if !truthy && !falsey {
        return;
    }
    // `begin ... end while cond` is a `while_post` upstream, whose body always runs once.
    let post = node
        .field("body")
        .filter(|body| body.kind_str() == "begin" && node.kind_str().ends_with("_modifier"));
    let keeps_running = truthy == is_while;
    let offense = report(condition.byte_range(), context);
    let edit = match (post, keeps_running) {
        (None, true) => Edit {
            start: condition.start_byte(),
            end: condition.end_byte(),
            replacement: keep.to_owned(),
            safe: true,
        },
        (None, false) => Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        },
        (Some(_), true) => Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: context.source.node_text(node).replacen(
                context.source.node_text(condition),
                keep,
                1,
            ),
            safe: true,
        },
        (Some(body), false) => Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: statements(body)
                .into_iter()
                .map(|statement| context.source.node_text(statement))
                .collect::<Vec<&str>>()
                .join("\n"),
            safe: true,
        },
    };
    offenses.push(
        offense
            .corrections_anchored_at(node.byte_range())
            .corrected_by(edit),
    );
}

/// `on_case`.
fn check_case(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if let Some(condition) = node.field("value") {
        if !truthy(condition, context) && !falsey(condition, context) {
            return;
        }
        check_case_condition(condition, context, offenses);
        return;
    }
    for branch in named_children(node)
        .into_iter()
        .filter(|child| child.kind_str() == "when")
    {
        let conditions: Vec<Node<'_>> = named_children(branch)
            .into_iter()
            .filter(|child| child.kind_str() == "pattern")
            .flat_map(named_children)
            .collect();
        if conditions.is_empty()
            || !conditions
                .iter()
                .all(|condition| literal(*condition, context))
        {
            continue;
        }
        let range = conditions[0].start_byte()..conditions[conditions.len() - 1].end_byte();
        offenses.push(report(range, context));
    }
}

/// `on_case_match`, which accepts a literal subject as long as some `in` binds a variable.
fn check_case_match(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(condition) = node.field("value") else {
        return;
    };
    if binds_a_match_variable(node) {
        return;
    }
    check_case_condition(condition, context, offenses);
}

/// `check_case`: a composite subject is only a condition when everything in it is a plain value.
fn check_case_condition(
    condition: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    if condition.kind_str() == "array" && !primitive_array(condition, context) {
        return;
    }
    if super::literals::literal_type(condition, context) == Some("dstr") {
        return;
    }
    handle_node(condition, context, offenses);
}

/// `primitive_array?`.
fn primitive_array(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    named_children(node).into_iter().all(|child| {
        if child.kind_str() == "array" {
            primitive_array(child, context)
        } else {
            is_basic_literal(child, context)
        }
    })
}

/// `descendants.any?(&:match_var_type?)`: a pattern that names what it matched.
fn binds_a_match_variable(node: Node<'_>) -> bool {
    for child in named_children(node) {
        if child.kind_str() == "in_clause" {
            if let Some(pattern) = child.field("pattern")
                && has_binding(pattern)
            {
                return true;
            }
            continue;
        }
        if binds_a_match_variable(child) {
            return true;
        }
    }
    false
}

fn has_binding(node: Node<'_>) -> bool {
    if matches!(node.kind_str(), "identifier" | "match_pattern" | "test_pattern") {
        return true;
    }
    named_children(node).into_iter().any(has_binding)
}

/// `on_if`, whose correction leaves only the branch the literal condition selects.
fn check_if(
    node: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    ignored: &mut Vec<Range<usize>>,
) {
    let Some(condition) = node.field("condition") else {
        return;
    };
    let truthy = truthy(condition, context);
    if !truthy && !falsey(condition, context) {
        return;
    }
    let is_unless = matches!(node.kind_str(), "unless" | "unless_modifier");
    // `condition_evaluation?`: which branch the parser's normalisation makes the surviving one.
    let result = if is_unless { !truthy } else { truthy };
    // A modifier keeps what it guards under `body`; only the block form has a `then` clause.
    let if_branch = Branch::of(node.field("consequence").or_else(|| node.field("body")));
    let alternative = node.field("alternative");
    let else_branch = Branch::of(alternative);
    let (if_range, else_range) = (branch_range(&if_branch), branch_range(&else_branch));
    let is_elsif = node.kind_str() == "elsif";
    // `else?` is true for an `elsif` clause as well, which is where it is stored.
    let has_else = alternative.is_some();
    let surviving = if result { &if_range } else { &else_range };
    if surviving.is_none() && (is_elsif || has_else) {
        return;
    }
    let elsif_conditional = alternative.is_some_and(|branch| branch.kind_str() == "elsif");
    let source = |range: &Range<usize>| context.source.slice(range.clone()).to_owned();
    let replacement = match (is_elsif, result) {
        (true, true) => format!("else\n  {}", source(&if_range.clone().unwrap_or(0..0))),
        (true, false) => format!("else\n  {}", source(&else_range.clone().unwrap_or(0..0))),
        _ => {
            if result && if_range.is_some() {
                source(&if_range.clone().unwrap_or(0..0))
            } else if elsif_conditional {
                let branch = source(&else_range.clone().unwrap_or(0..0));
                format!("{}\nend", branch.replacen("elsif", "if", 1))
            } else if has_else || node.kind_str() == "conditional" {
                source(&else_range.clone().unwrap_or(0..0))
            } else {
                String::new()
            }
        }
    };
    let offense = report(condition.byte_range(), context);
    let covered = ignored
        .iter()
        .any(|range| range.start <= node.start_byte() && node.end_byte() <= range.end);
    offenses.push(if covered {
        offense
    } else {
        ignored.push(node.byte_range());
        offense
            .corrections_anchored_at(node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            })
    });
}

/// The span of the node upstream puts in a branch: the one statement it holds, or the `begin`
/// around the several it holds.
fn branch_range(branch: &Branch<'_>) -> Option<Range<usize>> {
    match branch {
        Branch::Missing => None,
        Branch::One(node) => Some(node.byte_range()),
        Branch::Sequence(nodes) => Some(nodes[0].start_byte()..nodes[nodes.len() - 1].end_byte()),
    }
}
