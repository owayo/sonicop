use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children};

use super::blocks::{BLOCK_KINDS, BlockArgs};
use super::locals::LocalVariables;
use super::statements::statements;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for block in context.nodes_of_any(BLOCK_KINDS) {
        let Some(call) = block.parent_of(context) else {
            continue;
        };
        let Some(method) = call.field("method").map(|m| context.source.node_text(m)) else {
            continue;
        };
        if !matches!(method, "reduce" | "inject") {
            continue;
        }
        let Some(body) = block.field("body") else {
            continue;
        };
        // `node.argument_list.length >= 2`: without both an accumulator and an element there is
        // nothing to compare.
        let names = match BlockArgs::of(block, context, &locals) {
            BlockArgs::Written(params) => {
                let flattened = argument_list(&params, context);
                if flattened.len() < 2 {
                    continue;
                }
                flattened
            }
            BlockArgs::Numbered(highest) if highest >= 2 => {
                vec!["_1".to_owned(), "_2".to_owned()]
            }
            _ => continue,
        };
        let (accumulator, element) = (names[0].clone(), names[1].clone());
        check_return_values(
            context,
            offenses,
            block,
            body,
            method,
            &accumulator,
            &element,
        );
    }
}

/// `ArgsNode#argument_list`: a destructured parameter contributes the names it takes apart, not
/// itself.
fn argument_list(parameters: &[Node<'_>], context: &RuleContext<'_>) -> Vec<String> {
    let mut names = Vec::new();
    for parameter in parameters {
        if parameter.kind_str() == "destructured_parameter" {
            names.extend(argument_list(&named_children(*parameter), context));
            continue;
        }
        let node = parameter.field("name").unwrap_or(*parameter);
        names.push(context.source.node_text(node).to_owned());
    }
    names
}

fn check_return_values(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    block: Node<'_>,
    body: Node<'_>,
    method: &str,
    accumulator: &str,
    element: &str,
) {
    let values = return_values(block, body, context);
    if let Some(node) = values
        .iter()
        .find(|value| returns_accumulator_index(**value, accumulator, element, context))
    {
        offenses.push(context.offense(
            format!("Do not return an element of the accumulator in `{method}`."),
            node.byte_range(),
        ));
        return;
    }
    // `potential_offense?`: the element is never modified, and no branch hands the accumulator
    // back.
    if element_modified(body, element, context)
        || values
            .iter()
            .any(|value| lvar_used(*value, accumulator, context))
    {
        return;
    }
    for value in values {
        if acceptable_return(value, element, context) {
            continue;
        }
        offenses.push(context.offense(
            format!("Ensure the accumulator `{accumulator}` will be modified by `{method}`."),
            value.byte_range(),
        ));
    }
}

/// `return_values`: the last statement of the body, and whatever the block's own `next` and
/// `break` hand back.
fn return_values<'tree>(
    block: Node<'tree>,
    body: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Vec<Node<'tree>> {
    let mut values = Vec::new();
    let statements = statements(body);
    match statements.last() {
        Some(last) => values.push(*last),
        None => return values,
    }
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if matches!(node.kind_str(), "next" | "break")
            && enclosing_block(node, context).is_some_and(|inner| inner.id() == block.id())
            && let Some(argument) = handed_back(node)
        {
            values.push(argument);
        }
        crate::rules::push_named_children(node, &mut stack);
    }
    values
}

/// What a `next` or `break` hands back. The grammar wraps it in an argument list, which upstream has
/// no node for -- the value is the keyword's own child there.
fn handed_back<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let first = named_children(node).into_iter().next()?;
    match first.kind_str() {
        "argument_list" => named_children(first).into_iter().next(),
        _ => Some(first),
    }
}

fn enclosing_block<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if BLOCK_KINDS.contains(&ancestor.kind_str()) || ancestor.kind_str() == "lambda" {
            return Some(ancestor);
        }
        current = ancestor.parent_of(context);
    }
    None
}

/// `returned_accumulator_index`: `acc[el]` written back, which returns part of the accumulator
/// rather than the accumulator.
fn returns_accumulator_index(
    node: Node<'_>,
    accumulator: &str,
    element: &str,
    context: &RuleContext<'_>,
) -> bool {
    // `(send (lvar %1) {:[] :[]=} ...)`, which the grammar spells as an index or an assignment.
    let (index, assignment) = match node.kind_str() {
        "element_reference" => (node, false),
        "assignment" => match node.field("left") {
            Some(left) if left.kind_str() == "element_reference" => (left, true),
            _ => return false,
        },
        _ => return false,
    };
    if index
        .field("object")
        .is_none_or(|object| !is_lvar_named(object, accumulator, context))
    {
        return false;
    }
    if assignment {
        return true;
    }
    // The read is only an offence when nothing about the element decides which part is read.
    !index_arguments(index)
        .into_iter()
        .any(|argument| lvar_used(argument, element, context))
}

fn index_arguments<'tree>(index: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(index)
        .into_iter()
        .filter(|child| {
            child.kind_str() != "comment"
                && index
                    .field("object")
                    .is_none_or(|object| object.id() != child.id())
        })
        .collect()
}

/// `lvar_used?`: the name is handed back as it stands, written to, or appended to.
fn lvar_used(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    if is_lvar_named(node, name, context) {
        return true;
    }
    match node.kind_str() {
        "assignment" | "operator_assignment" => node
            .field("left")
            .is_some_and(|left| is_lvar_named(left, name, context)),
        "binary" => {
            node.child(1)
                .is_some_and(|operator| context.source.node_text(operator) == "<<")
                && node
                    .field("left")
                    .is_some_and(|left| is_lvar_named(left, name, context))
        }
        "call" => {
            node.field("method")
                .is_some_and(|method| context.source.node_text(method) == "<<")
                && node
                    .field("receiver")
                    .is_some_and(|receiver| is_lvar_named(receiver, name, context))
        }
        // `(dstr (begin (lvar %1)))`: a string holding nothing but the variable.
        "string" => {
            let parts: Vec<Node<'_>> = named_children(node)
                .into_iter()
                .filter(|child| child.kind_str() != "comment")
                .collect();
            matches!(parts.as_slice(), [only] if only.kind_str() == "interpolation"
                && matches!(interpolation_values(*only).as_slice(),
                    [inner] if is_lvar_named(*inner, name, context)))
        }
        _ => false,
    }
}

fn interpolation_values<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

fn is_lvar_named(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "identifier" && context.source.node_text(node) == name
}

/// `element_modified?`: the block does something to the element rather than to the accumulator.
fn element_modified(body: Node<'_>, element: &str, context: &RuleContext<'_>) -> bool {
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if modifies_element(node, element, context) {
            return true;
        }
        crate::rules::push_named_children(node, &mut stack);
    }
    false
}

fn modifies_element(node: Node<'_>, element: &str, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        // `(lvasgn %1 _)` and `(%SHORTHAND_ASSIGNMENTS (lvasgn %1) ... _)`.
        "assignment" | "operator_assignment" => node
            .field("left")
            .is_some_and(|left| is_lvar_named(left, element, context)),
        "call" => {
            let Some(method) = node.field("method") else {
                return false;
            };
            let name = context.source.node_text(method);
            let call_arguments = arguments(node);
            // `(send (lvar %1) _message <{ivar gvar cvar lvar send} ...>)`: the element is the
            // receiver of a call taking something.
            if node
                .field("receiver")
                .is_some_and(|receiver| is_lvar_named(receiver, element, context))
            {
                return call_arguments.iter().any(|argument| {
                    matches!(
                        argument.first().kind_str(),
                        "identifier"
                            | "instance_variable"
                            | "global_variable"
                            | "class_variable"
                            | "call"
                    )
                });
            }
            // `(send _receiver !{:[] :[]=} <`(lvar %1) `_ ...>)`: the element is handed to a call
            // alongside something else.
            if matches!(name, "[]" | "[]=") || call_arguments.len() < 2 {
                return false;
            }
            call_arguments
                .iter()
                .any(|argument| holds_lvar(argument.first(), element, context))
        }
        _ => false,
    }
}

fn holds_lvar(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if is_lvar_named(current, name, context) {
            return true;
        }
        crate::rules::push_named_children(current, &mut stack);
    }
    false
}

/// `acceptable_return?`: the expression reads something other than the element, so it may well be
/// building on the accumulator.
fn acceptable_return(node: Node<'_>, element: &str, context: &RuleContext<'_>) -> bool {
    let values = expression_values(node, context);
    values.is_empty() || values.iter().any(|value| value != element)
}

/// `expression_values`: the names an expression reads, and the bare calls it makes.
///
/// The method of a call is a node here and a symbol upstream, so it is stepped over rather than
/// counted as a name of its own.
fn expression_values(node: Node<'_>, context: &RuleContext<'_>) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind_str() {
            // `%VARIABLES`, and the bare receiverless call a plain name is when it was never
            // assigned -- both of which the search captures by source.
            "identifier" | "instance_variable" | "global_variable" | "class_variable" => {
                found.push(context.source.node_text(current).to_owned());
            }
            "assignment" | "operator_assignment" => {
                if let Some(left) = current.field("left") {
                    found.push(context.source.node_text(left).to_owned());
                }
            }
            // `$(send _ _)`: a call with no arguments at all, which the pattern captures whole.
            "call" if arguments(current).is_empty() && current.field("block").is_none() => {
                found.push(context.source.node_text(current).to_owned());
            }
            _ => {}
        }
        let method = (current.kind_str() == "call")
            .then(|| current.field("method"))
            .flatten();
        for child in named_children(current) {
            if method.is_some_and(|method| method.id() == child.id()) {
                continue;
            }
            stack.push(child);
        }
    }
    found.dedup();
    found
}
