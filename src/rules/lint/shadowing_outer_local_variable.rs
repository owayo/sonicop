use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::top_level_constant;

use super::statements::statements;
use super::variable_force::{Analysis, Declaration, Scope, Variable};

/// The node kinds `push_scope` opens a scope for, and whether the scope is a block -- which is the
/// only kind `VariableTable#find_variable` is allowed to look out of.
fn scope_kind(kind: &str) -> Option<bool> {
    match kind {
        // The file itself is the outermost scope, which `push_scope(root, true)` opens.
        "program" | "method" | "singleton_method" | "class" | "module" | "singleton_class" => {
            Some(false)
        }
        "block" | "do_block" | "lambda" => Some(true),
        _ => None,
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let analysis = context.variable_analysis();
    for variable in &analysis.variables {
        // `before_declaring_variable` only reaches a name the table did not already hold, and a
        // plain assignment to a name in scope is a write rather than a declaration. What is left
        // is the parameters and block locals a block always declares afresh.
        if !matches!(
            variable.kind,
            Declaration::Argument(_) | Declaration::BlockLocal
        ) || variable.should_be_unused()
        {
            continue;
        }
        let scope = &analysis.scopes[variable.scope];
        if scope_kind(scope.node.kind_str()) != Some(true) || is_ractor_block(scope.node, context) {
            continue;
        }
        let Some(outer) = find_outer(context, analysis, scope, variable) else {
            continue;
        };
        if used_in_declaration_of_outer(scope.node, outer, context)
            || same_conditions_different_branch(scope.node, outer, context)
        {
            continue;
        }
        offenses.push(context.offense(
            format!("Shadowing outer local variable - `{}`.", variable.name),
            variable.declaration.byte_range(),
        ));
    }
}

/// `VariableTable#find_variable`: the innermost scope holding the name, giving up as soon as a
/// scope that is not a block has been looked in.
fn find_outer<'tree, 'a>(
    context: &RuleContext<'_>,
    analysis: &'a Analysis<'tree>,
    scope: &Scope<'tree>,
    variable: &Variable<'tree>,
) -> Option<&'a Variable<'tree>> {
    let mut current = Some(scope.node);
    while let Some(node) = current {
        let block = scope_kind(node.kind_str())?;
        if let Some(found) = analysis
            .variables
            .iter()
            .filter(|other| other.name == variable.name)
            .filter(|other| analysis.scopes[other.scope].node.id() == node.id())
            // The table only holds what the walk has already reached.
            .find(|other| other.declaration.start_byte() < variable.declaration.start_byte())
        {
            return Some(found);
        }
        if !block {
            return None;
        }
        current = enclosing_scope_node(node, context);
    }
    None
}

fn enclosing_scope_node<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if scope_kind(ancestor.kind_str()).is_some() {
            return Some(ancestor);
        }
        current = ancestor.parent_of(context);
    }
    None
}

/// `ractor_block?`: `Ractor.new { }` runs somewhere else entirely, so a name of its own is no
/// shadow.
fn is_ractor_block(scope_node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(call) = scope_node.parent_of(context) else {
        return false;
    };
    call.kind_str() == "call"
        && call
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "new")
        && call
            .field("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "Ractor", context))
}

/// `variable_used_in_declaration_of_outer?`: the block stands inside the very expression that
/// declared the outer name, so the outer one is not in scope yet where it matters.
fn used_in_declaration_of_outer(
    scope_node: Node<'_>,
    outer: &Variable<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let mut current = scope_node.parent_of(context);
    while let Some(ancestor) = current {
        if ancestor.id() == outer.declaration.id() {
            return true;
        }
        current = ancestor.parent_of(context);
    }
    false
}

/// `same_conditions_node_different_branch?`: the two names live in branches of one conditional and
/// can never be in scope together.
fn same_conditions_different_branch(
    scope_node: Node<'_>,
    outer: &Variable<'_>,
    context: &RuleContext<'_>,
) -> bool {
    if different_case_in_branch(scope_node, outer, context) {
        return true;
    }
    // `variable_node`: what the block hangs off, with a `when` standing for the `case` it belongs
    // to. The grammar wraps the block in the call, which is the node upstream's `block` is.
    let Some(call) = scope_node.parent_of(context) else {
        return false;
    };
    let Some(mut variable_node) = upstream_parent(call, context) else {
        return false;
    };
    if variable_node.kind_str() == "when" {
        let Some(parent) = variable_node.parent_of(context) else {
            return false;
        };
        variable_node = parent;
    }
    if !is_conditional(variable_node) && conditional_ancestor(variable_node, context).is_none() {
        return false;
    }
    let Some(outer_node) = conditional_ancestor(outer.declaration, context) else {
        return false;
    };
    if variable_node.id() == outer_node.id() {
        return true;
    }
    matches!(outer_node.kind_str(), "if" | "unless" | "elsif")
        && else_branch(outer_node).is_some_and(|branch| branch.id() == variable_node.id())
}

/// `different_case_in_branch?`: two `in` branches of one `case`.
fn different_case_in_branch(
    scope_node: Node<'_>,
    outer: &Variable<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let (Some(inner), Some(outer_branch)) = (
        ancestor_of_kind(scope_node, "in_clause", context),
        ancestor_of_kind(outer.declaration, "in_clause", context),
    ) else {
        return false;
    };
    inner.id() != outer_branch.id()
        && inner
            .parent_of(context)
            .zip(outer_branch.parent_of(context))
            .is_some_and(|(one, other)| one.id() == other.id())
}

fn ancestor_of_kind<'tree>(
    node: Node<'tree>,
    kind: &str,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if ancestor.kind_str() == kind {
            return Some(ancestor);
        }
        current = ancestor.parent_of(context);
    }
    None
}

/// `node.parent`, as upstream's tree has it: a body holding one statement *is* that statement
/// there, so the container the grammar puts in between is stepped over.
fn upstream_parent<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context)?;
    while matches!(
        current.kind_str(),
        "then" | "else" | "body_statement" | "block_body" | "do"
    ) && statements(current).len() == 1
    {
        current = current.parent_of(context)?;
    }
    Some(current)
}

/// `find_conditional_node_from_ascendant`.
fn conditional_ancestor<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = upstream_parent(node, context);
    while let Some(ancestor) = current {
        if is_conditional(ancestor) {
            return Some(ancestor);
        }
        current = upstream_parent(ancestor, context);
    }
    None
}

/// `CONDITIONALS`.
fn is_conditional(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "if" | "unless"
            | "elsif"
            | "while"
            | "until"
            | "if_modifier"
            | "unless_modifier"
            | "while_modifier"
            | "until_modifier"
            | "conditional"
            | "case"
            | "case_match"
    )
}

/// `IfNode#else_branch`: the statement the `else` holds, or the clause itself when it holds more.
fn else_branch<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let alternative = node.field("alternative")?;
    if alternative.kind_str() != "else" {
        return Some(alternative);
    }
    let held = statements(alternative);
    match held.as_slice() {
        [only] => Some(*only),
        _ => Some(alternative),
    }
}
