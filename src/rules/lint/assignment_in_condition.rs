use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_WITH_SAFE_ASSIGNMENT_ALLOWED: &str = "Use `==` if you meant to do a comparison or wrap the expression in parentheses to \
     indicate you meant to assign in a condition.";
const MSG_WITHOUT_SAFE_ASSIGNMENT_ALLOWED: &str =
    "Use `==` if you meant to do a comparison or move the assignment up out of the condition.";

/// The nodes RuboCop's `on_if` / `on_while` / `on_until` fire for. Upstream reads `elsif`, `unless`
/// and the ternary as `if` nodes and the modifier forms as the loops they abbreviate, so all of
/// them are inspected -- but a `begin ... end while` is a `while_post`, which has no callback.
const CONDITIONALS: &[&str] = &[
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

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_safe: bool = context.setting("AllowSafeAssignment").unwrap_or(true);
    // Upstream fires the same callback for a node reachable from two conditions -- an `if` written
    // inside another condition is one -- and `add_offense` keeps a set of the ranges it has
    // already reported.
    let mut reported: HashSet<usize> = HashSet::new();
    for node in context.nodes_of_any(CONDITIONALS) {
        if post_condition_loop(node) {
            continue;
        }
        let Some(condition) = node.field("condition") else {
            continue;
        };
        traverse(condition, context, allow_safe, &mut reported, offenses);
    }
}

/// `begin ... end while cond` is a `while_post` upstream, which the cop has no callback for.
fn post_condition_loop(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "while_modifier" | "until_modifier")
        && node
            .field("body")
            .is_some_and(|body| body.kind_str() == "begin")
}

/// What one node of a condition is, in terms of the node types upstream reasons about.
enum Shape {
    /// `begin`: a parenthesized expression, or the code of an interpolation. Never reported
    /// itself, but decides whether what it holds is a deliberate "safe" assignment.
    Begin,
    /// An `=` assignment of any kind, including the setter-method call `foo.bar = 1` that upstream
    /// reads as a `send`.
    Assignment,
    /// A `send` or `csend` that is not an assignment method. Upstream stops the walk here, so an
    /// assignment written as an argument is not a condition and is not reported.
    Call,
    /// A block, which upstream refuses to walk into at all.
    Block,
    Other,
}

fn shape(node: Node<'_>, context: &RuleContext<'_>) -> Shape {
    match node.kind_str() {
        // `defined?(x = 1)`'s parentheses belong to the operator, so upstream has no `begin` node
        // between the two and the assignment is the condition itself. Write a space before them
        // and they are ordinary parentheses again.
        "parenthesized_statements" if defined_argument_parentheses(node) => Shape::Other,
        "parenthesized_statements" | "interpolation" => Shape::Begin,
        "assignment" if match_operator(node, context) => Shape::Call,
        "assignment" => Shape::Assignment,
        "block" | "do_block" | "lambda" => Shape::Block,
        // `super(...)` and `yield(...)` are calls here but nodes of their own upstream, so what
        // they are passed is still part of the condition.
        "call" => match node.field("method") {
            Some(method) if method.kind_str() == "super" => Shape::Other,
            _ => call_unless_assignment_target(node),
        },
        // `a[i]` is a `send` of `[]`, but `a[i] = 1` is a `send` of `[]=` -- an assignment method,
        // whose subscripts upstream does walk.
        "element_reference" => call_unless_assignment_target(node),
        // `!x` and `-x` are sends; `defined?(x)` is a node of its own.
        "unary" => match node.field("operator").map(|op| op.kind_str()) {
            Some("defined?") => Shape::Other,
            _ => Shape::Call,
        },
        // `&&` and `||` are `and` / `or` nodes upstream; every other operator is a send, except a
        // `=~` written against a regexp literal, which is a `match_with_lvasgn` -- the match can
        // bind local variables, so what stands on its right is still part of the condition.
        "binary" => match node.field("operator").map(|op| op.kind_str()) {
            Some("&&" | "||" | "and" | "or") => Shape::Other,
            Some("=~")
                if node
                    .field("left")
                    .is_some_and(|left| left.kind_str() == "regex") =>
            {
                Shape::Other
            }
            _ => Shape::Call,
        },
        _ => Shape::Other,
    }
}

/// Whether an `assignment` node is really a `=~` match. The grammar reads `a[i] =~ re` as an
/// assignment of `~re` -- only the subscript form of the left-hand side makes the operator
/// ambiguous to it. Ruby's lexer decides by adjacency: a `~` written against the `=` is the second
/// half of `=~`, and a space before it makes it the unary operator of the assigned value.
fn match_operator(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(operator) = assignment_operator(node) else {
        return false;
    };
    context.source.text().as_bytes().get(operator.end_byte()) == Some(&b'~')
}

fn defined_argument_parentheses(node: Node<'_>) -> bool {
    let Some(parent) = node.parent().filter(|parent| parent.kind_str() == "unary") else {
        return false;
    };
    let Some(operator) = parent
        .field("operator")
        .filter(|operator| operator.kind_str() == "defined?")
    else {
        return false;
    };
    operator.end_byte() == node.start_byte()
}

/// The left-hand side of an assignment is not a call of its own upstream: it is part of the one
/// `send` that does the assigning, whose children the walk goes on to visit.
fn call_unless_assignment_target(node: Node<'_>) -> Shape {
    let target = node.parent().is_some_and(|parent| {
        parent.kind_str() == "assignment"
            && parent
                .field("left")
                .is_some_and(|left| left.id() == node.id())
    });
    if target { Shape::Other } else { Shape::Call }
}

fn traverse(
    node: Node<'_>,
    context: &RuleContext<'_>,
    allow_safe: bool,
    reported: &mut HashSet<usize>,
    offenses: &mut Vec<Offense>,
) {
    match shape(node, context) {
        Shape::Block | Shape::Call => return,
        Shape::Begin => {
            let statements = statements(node);
            // `()` holds no condition at all, and `(x = 1)` is the parenthesized form that says
            // the assignment was meant.
            if statements.is_empty()
                || (allow_safe
                    && statements.len() == 1
                    && matches!(shape(statements[0], context), Shape::Assignment))
            {
                return;
            }
        }
        Shape::Assignment => {
            if !discarded(node)
                && let Some(operator) = assignment_operator(node)
                && reported.insert(operator.start_byte())
            {
                let message = if allow_safe {
                    MSG_WITH_SAFE_ASSIGNMENT_ALLOWED
                } else {
                    MSG_WITHOUT_SAFE_ASSIGNMENT_ALLOWED
                };
                let offense = context.offense(message, operator.byte_range());
                offenses.push(match correction(context, node, allow_safe) {
                    Some(edit) => offense.corrected_by(edit),
                    None => offense,
                });
            }
        }
        Shape::Other => {}
    }
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
    for child in children {
        traverse(child, context, allow_safe, reported, offenses);
    }
}

/// An assignment that is one statement of a multi-statement `(...)` has its value thrown away, so
/// it is not what the condition tests.
fn discarded(node: Node<'_>) -> bool {
    node.parent()
        .filter(|parent| matches!(parent.kind_str(), "parenthesized_statements" | "interpolation"))
        .is_some_and(|parent| statements(parent).len() > 1)
}

/// The statements a `(...)` or `#{...}` holds. A `;` and the two kinds the grammar hangs off the
/// statement that mentioned them are not statements of it.
fn statements(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "empty_statement" | "comment" | "heredoc_body"))
        .collect()
}

/// The `=` the offense is reported at, which is `loc.operator` upstream.
fn assignment_operator<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).find(|child| child.kind_str() == "=")
}

/// Wrapping the assignment in parentheses is the one correction: it says the assignment was meant.
/// With `AllowSafeAssignment: false` that is no longer an answer, and upstream leaves the
/// corrector empty.
fn correction(context: &RuleContext<'_>, node: Node<'_>, allow_safe: bool) -> Option<Edit> {
    if !allow_safe {
        return None;
    }
    let range: Range<usize> = node.byte_range();
    Some(Edit {
        start: range.start,
        end: range.end,
        replacement: format!("({})", context.source.node_text(node)),
        safe: true,
    })
}
