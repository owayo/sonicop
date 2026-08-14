use tree_sitter::Node;

use super::complexity::{Allowed, CsendDiscount, Emit, Kind, Order, Walk, measured};
use super::cyclomatic_complexity::score_for;
use super::locals::{named_children, operator};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: i64 = context.setting("Max").unwrap_or(8);
    let allowed = Allowed::new(context);
    let methods = measured(context, &allowed);
    if methods.is_empty() {
        return;
    }
    let fragments = context.fragments();
    let locals = context.metric_locals();
    let walk = Walk::new(context, locals, fragments, Order::Pre);
    for method in methods {
        let mut score = 1i64;
        let mut discount = CsendDiscount::default();
        walk.run(method.body, &mut |emit| {
            if emit.kind == Kind::Lvasgn {
                discount.reset(emit.name);
            } else if is_counted(emit.kind) {
                score += perceived_score(emit, &mut discount);
            }
        });
        if score <= max {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "Perceived complexity for `{}` is too high. [{score}/{max}]",
                method.name
            ),
            method.location.byte_range(),
        ));
    }
}

/// `PerceivedComplexity#complexity_score_for`.
fn perceived_score<'a>(emit: Emit<'a>, discount: &mut CsendDiscount<'a>) -> i64 {
    match emit.kind {
        Kind::Case => case_score(emit.node),
        Kind::CaseMatch => case_match_score(emit.node),
        // An `else` costs as much as the branch it stands against, but an `elsif` is one of a
        // chain and is counted where it is written instead.
        Kind::If => {
            let has_else = matches!(emit.node.kind_str(), "if" | "elsif" | "unless")
                && emit.node.field("alternative").is_some();
            if has_else && emit.node.kind_str() != "elsif" {
                2
            } else {
                1
            }
        }
        _ => score_for(emit, discount),
    }
}

/// A `case` with no expression after it is an `if`/`elsif` chain in disguise, so every `when`
/// counts; one with an expression takes 0.8 points itself and gives each branch 0.2.
fn case_score(node: Node<'_>) -> i64 {
    let branches = named_children(node)
        .iter()
        .filter(|child| child.kind_str() == "when")
        .count() as i64
        + i64::from(has_body(else_clause(node)));
    if node.field("value").is_none() {
        return branches;
    }
    round_tenths(branches * 2 + 8)
}

/// An `in` branch whose pattern is a plain literal or a constant reads like a `when`, so it is
/// discounted the same way; a structural pattern is a decision point in full.
fn case_match_score(node: Node<'_>) -> i64 {
    let mut tenths: i64 = named_children(node)
        .iter()
        .filter(|child| child.kind_str() == "in_clause")
        .map(|clause| if simple_pattern(*clause) { 2 } else { 10 })
        .sum();
    // A `case`/`in` with an empty `else` still builds an `empty_else` node, which counts.
    if node.field("else").is_some() {
        tenths += 2;
    }
    round_tenths(tenths)
}

/// `Float#round` on a value that is a whole number of tenths. No such value ever falls exactly
/// half way, since a tenth of a point is always an even digit, so rounding up is unambiguous.
fn round_tenths(tenths: i64) -> i64 {
    (tenths + 5) / 10
}

fn simple_pattern(clause: Node<'_>) -> bool {
    if clause.field("guard").is_some() {
        return false;
    }
    clause
        .field("pattern")
        .is_some_and(is_literal_or_constant)
}

/// `Node#literal?` or `Node#const_type?`, for the node types a pattern can hold.
fn is_literal_or_constant(node: Node<'_>) -> bool {
    match node.kind_str() {
        "string" | "chained_string" | "subshell" | "integer" | "float" | "rational" | "complex"
        | "character" | "simple_symbol" | "delimited_symbol" | "bare_symbol" | "array" | "hash"
        | "regex" | "range" | "true" | "false" | "nil" | "constant" | "scope_resolution" => true,
        // `in -1` is one negative literal upstream rather than a call to `-@`.
        "unary" => {
            matches!(operator(node), Some("-" | "+"))
                && node.field("operand").is_some_and(|operand| {
                    matches!(operand.kind_str(), "integer" | "float" | "rational")
                })
        }
        _ => false,
    }
}

fn else_clause<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind_str() == "else")
}

/// Whether an `else` holds a statement. An empty one leaves `else_branch` nil upstream, and a
/// branch that is not there is not counted.
fn has_body(clause: Option<Node<'_>>) -> bool {
    clause.is_some_and(|clause| {
        named_children(clause)
            .iter()
            .any(|child| !matches!(child.kind_str(), "comment" | "empty_statement"))
    })
}

/// `PerceivedComplexity::COUNTED_NODES`: the cyclomatic list without `when` and `in_pattern`, and
/// with the `case` nodes those belonged to instead.
fn is_counted(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::If
            | Kind::While
            | Kind::Until
            | Kind::For
            | Kind::Csend
            | Kind::Block
            | Kind::BlockPass
            | Kind::Rescue
            | Kind::And
            | Kind::Or
            | Kind::OrAsgn
            | Kind::AndAsgn
            | Kind::Case
            | Kind::CaseMatch
    )
}
