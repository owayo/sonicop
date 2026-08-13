use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;

const MSG: &str =
    "Consider using explicit block argument in the surrounding method's signature over `yield`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // Upstream keeps the definitions it has already given a block parameter for the whole file, so
    // two blocks in one method only add it once.
    let mut named: HashSet<usize> = HashSet::new();
    for node in context.nodes_of("yield") {
        let Some((call, block)) = yielding_block(node) else {
            continue;
        };
        let block_parameters = block
            .child_by_field_name("parameters")
            .map(super::nodes::children)
            .unwrap_or_default();
        // The grammar hangs a `yield`'s arguments off it without a field name.
        let yield_arguments = super::nodes::children(node)
            .into_iter()
            .find(|child| child.kind() == "argument_list")
            .map(super::nodes::children)
            .unwrap_or_default();
        if !yields_its_arguments(context, &block_parameters, &yield_arguments) {
            continue;
        }
        let Some(definition) = enclosing_definition(node) else {
            continue;
        };
        let name = block_name(context, definition);
        let mut edits = vec![Edit {
            start: send_node::send_range(call, context).end,
            end: call.end_byte(),
            replacement: String::new(),
            safe: true,
        }];
        edits.extend(add_block_argument(context, call, &name, true));
        if named.insert(definition.id()) {
            edits.extend(add_block_argument(context, definition, &name, false));
        }
        offenses.push(
            context
                .offense(MSG, call.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `(block $_ (args $...) (yield $...))`: a block whose whole body is one `yield`.
fn yielding_block<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, Node<'tree>)> {
    let body = node.parent()?;
    if !matches!(body.kind(), "block_body" | "body_statement") {
        return None;
    }
    match super::nodes::children(body).as_slice() {
        [only] if only.id() == node.id() => {}
        _ => return None,
    }
    let block = body.parent()?;
    if !matches!(block.kind(), "block" | "do_block") {
        return None;
    }
    let call = block.parent()?;
    (call.kind() == "call").then_some((call, block))
}

/// `yielding_arguments?`: the block hands each of its parameters straight to `yield`, in order.
fn yields_its_arguments(
    context: &RuleContext<'_>,
    parameters: &[Node<'_>],
    arguments: &[Node<'_>],
) -> bool {
    if arguments.len() > parameters.len() {
        return false;
    }
    if parameters.is_empty() {
        return true;
    }
    parameters.len() == arguments.len()
        && parameters
            .iter()
            .zip(arguments)
            .all(|(parameter, argument)| {
                parameter.kind() == "identifier"
                    && argument.kind() == "identifier"
                    && context.source.node_text(*parameter) == context.source.node_text(*argument)
            })
}

/// The `def` the block is written inside, which is where the block parameter goes.
fn enclosing_definition<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(parent.kind(), "method" | "singleton_method") {
            return Some(parent);
        }
        current = parent.parent();
    }
    None
}

/// `extract_block_name`: a definition that already declares a block parameter keeps its name.
fn block_name(context: &RuleContext<'_>, definition: Node<'_>) -> String {
    definition
        .child_by_field_name("parameters")
        .map(super::nodes::children)
        .and_then(|parameters| parameters.last().copied())
        .filter(|last| last.kind() == "block_parameter")
        .and_then(|last| last.child_by_field_name("name"))
        .map_or_else(
            || "block".to_owned(),
            |name| context.source.node_text(name).to_owned(),
        )
}

/// `add_block_argument`: the parameter list gains `&block`, however it was written.
fn add_block_argument(
    context: &RuleContext<'_>,
    node: Node<'_>,
    name: &str,
    call_like: bool,
) -> Vec<Edit> {
    let field = match call_like {
        true => "arguments",
        false => "parameters",
    };
    let list = node.child_by_field_name(field);
    let written = list.map(super::nodes::children).unwrap_or_default();
    if let Some(last) = written.last() {
        // A block parameter already there needs nothing added.
        if last.kind() == "block_parameter" || last.kind() == "block_argument" {
            return Vec::new();
        }
        // `range_with_surrounding_comma(:right)`: a trailing comma is written over rather than
        // doubled.
        let text = context.source.text().as_bytes();
        let mut end = last.end_byte();
        let comma = text.get(end) == Some(&b',');
        if comma {
            end += 1;
        }
        let replacement = match comma {
            true => format!(" &{name}"),
            false => format!(", &{name}"),
        };
        return vec![Edit {
            start: end,
            end,
            replacement,
            safe: true,
        }];
    }
    if let Some(list) = list
        && context.source.node_text(list).starts_with('(')
    {
        return vec![Edit {
            start: list.start_byte(),
            end: list.end_byte(),
            replacement: format!("(&{name})"),
            safe: true,
        }];
    }
    let anchor = match call_like {
        // `correct_call_node`: the parentheses go after the whole call.
        true => send_node::send_range(node, context).end,
        false => match node.child_by_field_name("name") {
            Some(selector) => selector.end_byte(),
            None => return Vec::new(),
        },
    };
    vec![Edit {
        start: anchor,
        end: anchor,
        replacement: format!("(&{name})"),
        safe: true,
    }]
}
