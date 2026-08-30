use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::variable_force::{Analysis, Assignment, Declaration, Scope, Variable};
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

/// The scope kinds `method_argument?` and `block_argument?` accept: a `def`, a `defs`, and the
/// `block` a literal lambda is one of.
const ARGUMENT_SCOPES: &[&str] = &["method", "singleton_method", "block", "do_block", "lambda"];

/// `Node::CONDITIONALS`, in the kinds tree-sitter writes them as. A ternary is an `if` upstream.
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
    "case",
    "case_match",
];

/// `node.type?(:block, :rescue)`: a body that may or may not run.
const UNCERTAIN: &[&str] = &["block", "do_block", "lambda", "rescue"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_implicit = context
        .setting::<bool>("IgnoreImplicitReferences")
        .unwrap_or(false);
    let analysis = context.variable_analysis();
    for scope in &analysis.scopes {
        for &index in &scope.variables {
            let variable = &analysis.variables[index];
            if !is_argument(variable, scope) {
                continue;
            }
            if let Some(node) = shadowing_assignment(variable, scope, ignore_implicit, analysis, context) {
                offenses.push(context.offense(
                    format!(
                        "Argument `{}` was shadowed by a local variable before it was used.",
                        variable.name
                    ),
                    node.byte_range(),
                ));
            }
        }
    }
}

/// `argument.method_argument? || argument.block_argument?`, minus the block local variables that
/// `explicit_block_local_variable?` excludes.
fn is_argument(variable: &Variable<'_>, scope: &Scope<'_>) -> bool {
    matches!(variable.kind, Declaration::Argument(_))
        && ARGUMENT_SCOPES.contains(&scope.node.kind_str())
}

/// `shadowing_assignment`: the write that replaced the argument before anything read it.
fn shadowing_assignment<'tree>(
    variable: &'tree Variable<'tree>,
    scope: &Scope<'tree>,
    ignore_implicit: bool,
    analysis: &Analysis<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if variable.references.is_empty() {
        return None;
    }
    let (node, location_known) = assignment_without_argument_usage(variable, scope, analysis, context)?;
    let start = node.start_byte();
    let consumed: Vec<std::ops::Range<usize>> = variable
        .assignments
        .iter()
        .flat_map(|assignment| assignment.references.iter())
        .map(|reference| reference.byte_range())
        .collect();
    // `argument_references`: the reads that took the argument's own value rather than what a
    // later write put there.
    let used_first = variable
        .references
        .iter()
        .filter(|reference| {
            !(reference.explicit && consumed.contains(&reference.node.byte_range()))
        })
        .any(|reference| {
            (!reference.explicit && ignore_implicit) || reference_position(reference.node) <= start
        });
    if used_first {
        return None;
    }
    Some(if location_known {
        node
    } else {
        variable.declaration
    })
}

/// `assignment_without_argument_usage`: the first write whose value does not read the argument,
/// and whether the place it was written at is decidable.
fn assignment_without_argument_usage<'tree>(
    variable: &'tree Variable<'tree>,
    scope: &Scope<'tree>,
    analysis: &Analysis<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, bool)> {
    let mut location_known = true;
    for assignment in &variable.assignments {
        let node = meta_assignment_node(assignment);
        // A shorthand assignment always reads what it writes.
        if node.kind_str() == "operator_assignment" {
            location_known = false;
            continue;
        }
        let Some(parent) = node.parent_of(context) else {
            location_known = false;
            continue;
        };
        if uses_variable(node, &variable.name, analysis, context) {
            continue;
        }
        if conditional_assignment(parent, scope.node) {
            location_known = false;
            continue;
        }
        return Some((assignment.node, location_known));
    }
    None
}

/// `assignment.meta_assignment_node || assignment.node`: the expression the write is part of, which
/// for a multiple or operator assignment is bigger than the write itself.
fn meta_assignment_node<'tree>(assignment: &Assignment<'tree>) -> Node<'tree> {
    let node = assignment.node;
    if node.kind_str() == "assignment" {
        return node;
    }
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind_str() {
            "assignment" | "operator_assignment" | "for" => return parent,
            "left_assignment_list" | "rest_assignment" | "splat_argument" => current = parent,
            _ => break,
        }
    }
    node
}

/// `uses_var?(assignment_node, name)`: whether the write reads the variable it writes.
fn uses_variable(
    node: Node<'_>,
    name: &str,
    analysis: &Analysis<'_>,
    context: &RuleContext<'_>,
) -> bool {
    if node.kind_str() == "identifier"
        && analysis.is_variable_reference(node)
        && context.source.node_text(node) == name
    {
        return true;
    }
    // A heredoc's body is a child of the string upstream and a node of its own here, so the read
    // written inside `x = <<~TEXT ... \#{x} ... TEXT` is not reachable from the assignment.
    if node.kind_str() == "heredoc_beginning"
        && let Some(body) = crate::rules::send_node::heredoc_body(node, context)
        && uses_variable(body, name, analysis, context)
    {
        return true;
    }
    named_children_of(node, context)
        .into_iter()
        .any(|child| uses_variable(child, name, analysis, context))
}

/// `conditional_assignment?`: whether anything between the write and the scope it belongs to may
/// keep the write from running.
fn conditional_assignment(node: Node<'_>, stop: Node<'_>) -> bool {
    let mut current = node;
    loop {
        if current.id() == stop.id() {
            return false;
        }
        if CONDITIONALS.contains(&current.kind_str()) || UNCERTAIN.contains(&current.kind_str()) {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// `reference_pos`: a read written as one target of a multiple assignment is placed at the whole
/// assignment rather than at the target.
fn reference_position(node: Node<'_>) -> usize {
    match node.parent() {
        Some(parent)
            if parent.kind_str() == "assignment"
                && parent
                    .field("left")
                    .is_some_and(|left| left.kind_str() == "left_assignment_list") =>
        {
            parent.start_byte()
        }
        _ => node.start_byte(),
    }
}
