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
        let Some(target) = yielding_block(node, context) else {
            continue;
        };
        let block_parameters = target
            .parameters
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
            start: target.send_end,
            end: target.node.end_byte(),
            replacement: String::new(),
            safe: true,
        }];
        edits.extend(add_block_argument(
            context,
            target.arguments,
            target.send_end,
            &name,
        ));
        if named.insert(definition.id()) {
            edits.extend(add_block_argument(
                context,
                definition.child_by_field_name("parameters"),
                definition
                    .child_by_field_name("name")
                    .map_or(definition.end_byte(), |name| name.end_byte()),
                &name,
            ));
        }
        offenses.push(
            context
                .offense(MSG, target.node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// What one recognized block gives the correction: where it is reported, where the call it hangs
/// off ends, and the parameters it declares.
struct Target<'tree> {
    node: Node<'tree>,
    send_end: usize,
    parameters: Option<Node<'tree>>,
    arguments: Option<Node<'tree>>,
}

/// `(block $_ (args $...) (yield $...))`: a block whose whole body is one `yield`.
fn yielding_block<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Target<'tree>> {
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
    let owner = block.parent()?;
    match owner.kind() {
        // `-> { yield }` is a block whose send is `(send nil :lambda)`, whose source is the `->`.
        "lambda" => Some(Target {
            node: owner,
            send_end: owner.child(0)?.end_byte(),
            parameters: owner.child_by_field_name("parameters"),
            arguments: None,
        }),
        "call" => Some(Target {
            node: owner,
            send_end: send_node::send_range(owner, context).end,
            parameters: block.child_by_field_name("parameters"),
            arguments: owner.child_by_field_name("arguments"),
        }),
        _ => None,
    }
}

/// `yielding_arguments?`: the block hands each of its parameters straight to `yield`, in order.
fn yields_its_arguments(
    context: &RuleContext<'_>,
    parameters: &[Node<'_>],
    arguments: &[Node<'_>],
) -> bool {
    // The yield arguments are padded with nils up to the parameter count, and a nil on either side
    // fails the comparison -- so the two lists have to be the same length.
    parameters.len() == arguments.len()
        && parameters
            .iter()
            .zip(arguments)
            .all(|(parameter, argument)| {
                argument.kind() == "identifier"
                    && parameter_name(context, *parameter)
                        == Some(context.source.node_text(*argument))
            })
}

/// The name a parameter declares, which is what upstream compares the yielded variable against.
fn parameter_name<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    match node.kind() {
        "identifier" => Some(context.source.node_text(node)),
        "splat_parameter" | "hash_splat_parameter" | "block_parameter" | "keyword_parameter"
        | "optional_parameter" => node
            .child_by_field_name("name")
            .map(|name| context.source.node_text(name)),
        _ => None,
    }
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
    list: Option<Node<'_>>,
    anchor: usize,
    name: &str,
) -> Vec<Edit> {
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
    vec![Edit {
        start: anchor,
        end: anchor,
        replacement: format!("(&{name})"),
        safe: true,
    }]
}
