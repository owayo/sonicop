use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use the double pipe equals operator `||=` instead.";

/// The left-hand sides the parser spells as a plain variable assignment, and the reads that match
/// them.
const VARIABLES: &[&str] = &[
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_lvasgn`: `name = name ? name : 'x'`.
    for node in context.nodes_of("assignment") {
        let (Some(left), Some(right)) = (node.field("left"), node.field("right")) else {
            continue;
        };
        // Upstream matches an `if` node here, and its parser builds one for the ternary *and* for
        // the keyword form. The grammar splits them, so both kinds have to be taken -- leaving the
        // keyword form out is the cop going quiet on `x = if x then x else 'default' end`.
        if !VARIABLES.contains(&left.kind_str())
            || !matches!(right.kind_str(), "conditional" | "if")
        {
            continue;
        }
        let name = context.source.node_text(left);
        let (Some(condition), Some(consequence), Some(alternative)) = (
            right.field("condition"),
            right.field("consequence").and_then(sole_statement),
            right.field("alternative").and_then(sole_statement),
        ) else {
            continue;
        };
        if !reads(context, condition, left, name) || !reads(context, consequence, left, name) {
            continue;
        }
        // `return if else_branch.if_type?`. The grammar gives `elsif` a kind of its own, and
        // upstream's parser builds an `if` for it, so it belongs in this list too.
        if matches!(
            alternative.kind_str(),
            "if" | "unless" | "elsif" | "conditional" | "if_modifier" | "unless_modifier"
        ) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: format!("{name} ||= {}", context.source.node_text(alternative)),
            safe: true,
        }));
    }

    // `on_if`: `unless_assignment?` is `(if cond nil? assignment)` -- a condition that reads the
    // variable, an empty then branch, and the assignment in the else. Upstream's parser reaches
    // that shape from `name = 'x' unless name` *and* from an `if` written with an empty body and
    // an `else`, because it has one node for both. The grammar keeps them apart.
    let mut locals = None;
    for node in context.nodes_of_any(&["unless", "unless_modifier", "if"]) {
        let empty_if = node.kind_str() == "if";
        if !empty_if && node.field("alternative").is_some() {
            continue;
        }
        // The `if` form only matches when the then branch holds nothing, which is what upstream
        // spells `nil?`.
        if empty_if
            && node
                .field("consequence")
                .is_some_and(|then| !super::nodes::children(then).is_empty())
        {
            continue;
        }
        let Some(condition) = node.field("condition") else {
            continue;
        };
        if !VARIABLES.contains(&condition.kind_str()) {
            continue;
        }
        // `{lvar ivar cvar gvar}`: a bare name is only one of those once it has been assigned, and
        // a first mention is a receiverless call. The modifier form assigns before the condition is
        // even read, so only the keyword form has to ask.
        if node.kind_str() == "unless"
            && condition.kind_str() == "identifier"
            && !locals
                .get_or_insert_with(|| LocalVariables::new(context))
                .is_lvar(condition)
        {
            continue;
        }
        // The `unless` forms carry the assignment in the body; the `if` form carries it in the
        // `else`, which is the same `(if cond nil? assignment)` upstream sees either way.
        let body = match empty_if {
            true => node.field("alternative").and_then(sole_statement),
            false => body_statement(node),
        };
        let Some(body) = body else {
            continue;
        };
        if body.kind_str() != "assignment" {
            continue;
        }
        let (Some(left), Some(right)) = (body.field("left"), body.field("right")) else {
            continue;
        };
        if left.kind_str() != condition.kind_str()
            || context.source.node_text(left) != context.source.node_text(condition)
        {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: format!(
                "{} ||= {}",
                context.source.node_text(left),
                context.source.node_text(right)
            ),
            safe: true,
        }));
    }
}

/// What a `then` or `else` clause holds, when it holds exactly one statement.
///
/// The ternary keeps its branches directly under the conditional, while the keyword form wraps
/// each in a clause node. Upstream's parser has neither wrapper, so the two shapes have to be
/// brought back together before the branches can be compared.
fn sole_statement<'tree>(clause: Node<'tree>) -> Option<Node<'tree>> {
    match clause.kind_str() {
        "then" | "else" => match super::nodes::children(clause).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        _ => Some(clause),
    }
}

/// `({lvar ivar cvar gvar} _var)`: a read of the very variable being assigned.
fn reads(context: &RuleContext<'_>, node: Node<'_>, left: Node<'_>, name: &str) -> bool {
    node.kind_str() == left.kind_str() && context.source.node_text(node) == name
}

/// The one statement the `unless` guards.
fn body_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    // The keyword form spells its body as a `then` clause; the modifier form has it directly.
    let body = node.field("body").or_else(|| node.field("consequence"))?;
    match body.kind_str() {
        "then" => match super::nodes::children(body).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        _ => Some(body),
    }
}
