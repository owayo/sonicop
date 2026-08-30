use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

use super::statements::statements;
use crate::rules::send_node::named_children_of;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("rescue") {
        // `resbody_branches.last`: only the clause that would have re-raised is useless; an
        // earlier one still keeps the later ones from being reached.
        if !is_last_branch(node, context) || !only_reraising(node, context) {
            continue;
        }
        offenses.push(context.offense("Useless `rescue` detected.", node.byte_range()));
    }
}

fn is_last_branch(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return true;
    };
    let mut seen = false;
    for sibling in named_children_of(parent, context) {
        if sibling.id() == node.id() {
            seen = true;
            continue;
        }
        if seen && sibling.kind_str() == "rescue" {
            return false;
        }
    }
    true
}

/// `only_reraising?`: the clause does nothing but raise the exception it caught.
fn only_reraising(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let variable = node
        .field("variable")
        .and_then(|variable| variable.named_child(0))
        .map(|name| context.source.node_text(name));
    if uses_exception_variable_in_ensure(node, variable, context) {
        return false;
    }
    let Some(body) = node.field("body") else {
        return false;
    };
    let body_statements = statements(body);
    let [statement] = body_statements.as_slice() else {
        return false;
    };
    let statement = *statement;
    // A bare `raise` is an identifier here rather than a call, and it takes no arguments.
    if statement.kind_str() == "identifier" {
        return context.source.node_text(statement) == "raise";
    }
    if statement.kind_str() != "call"
        || statement.field("receiver").is_some()
        || statement
            .field("method")
            .is_none_or(|method| context.source.node_text(method) != "raise")
    {
        return false;
    }
    let call_arguments = arguments(statement);
    match call_arguments.as_slice() {
        [] => true,
        // `exception_objects`: the caught exception, however it is named.
        [only] => {
            let source = context.source.slice(only.range());
            variable.is_some_and(|variable| variable == source)
                || matches!(source, "$!" | "$ERROR_INFO")
        }
        _ => false,
    }
}

/// `use_exception_variable_in_ensure?`: the clause is not useless when the `ensure` beside it
/// still reads the exception it named.
fn uses_exception_variable_in_ensure(
    node: Node<'_>,
    variable: Option<&str>,
    context: &RuleContext<'_>,
) -> bool {
    let Some(variable) = variable else {
        return false;
    };
    // The clause and the `ensure` are siblings here, where upstream nests the `rescue` inside the
    // `ensure` node -- so the ancestor the cop looks for is the sibling that follows.
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    let Some(ensure_clause) = named_children_of(parent, context)
        .into_iter()
        .find(|child| child.kind_str() == "ensure")
    else {
        return false;
    };
    let analysis = context.variable_analysis();
    let mut found = false;
    crate::rules::walk_named(ensure_clause, context, &mut |inner| {
        if found || inner.kind_str() != "identifier" || context.source.node_text(inner) != variable
        {
            return;
        }
        found = analysis.is_variable_reference(inner);
    });
    found
}
