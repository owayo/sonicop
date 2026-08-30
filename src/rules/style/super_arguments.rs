//! `Style/SuperArguments`: `super(a, b)` where bare `super` already passes the same arguments.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, send_range, symbol_name};

const MSG: &str = "Call `super` without arguments and parentheses when the signature is identical.";
const MSG_INLINE_BLOCK: &str = "Call `super` without arguments and parentheses when all positional \
                                and keyword arguments are forwarded.";

/// The kinds a block is written as, which end the search for the definition.
const BLOCKS: &[&str] = &["block", "do_block"];

/// `ASSIGN_TYPES`: what could give the block parameter another value before `super` runs.
const ASSIGN_TYPES: &[&str] = &["assignment", "operator_assignment"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        // A bare `super` is a `zsuper` upstream and never reaches this handler; only the form
        // written with parentheses is a `super` node.
        if !call
            .field("method")
            .is_some_and(|method| method.kind_str() == "super")
            || call.field("arguments").is_none()
        {
            continue;
        }
        let Some(definition) = find_def_node(call) else {
            continue;
        };
        let parameters = definition_parameters(definition, context);
        let super_arguments = preprocess_super_args(call);
        if !arguments_identical(context, call, definition, &parameters, &super_arguments) {
            continue;
        }
        let message = match parameters.len() == super_arguments.len() {
            true => MSG,
            false => MSG_INLINE_BLOCK,
        };
        // `super_node` stops before the block upstream, which is a node of its own there.
        let range = send_range(call, context);
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: "super".to_owned(),
            safe: true,
        }));
    }
}

/// `find_def_node`: the definition the `super` belongs to, unless a block of somebody else's
/// stands between them.
fn find_def_node<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = call.parent();
    while let Some(node) = current {
        if BLOCKS.contains(&node.kind_str()) {
            return None;
        }
        if matches!(node.kind_str(), "method" | "singleton_method") {
            return Some(node);
        }
        current = node.parent();
    }
    None
}

/// `def_node.arguments.argument_list`.
struct Parameter {
    kind: String,
    name: Option<String>,
}

fn definition_parameters(definition: Node<'_>, context: &RuleContext<'_>) -> Vec<Parameter> {
    let Some(list) = definition.field("parameters") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for child in super::nodes::children_in(list, context) {
        match unfolded_defaults(child) {
            Some(names) => found.extend(names.into_iter().map(|name| Parameter {
                kind: "optional_parameter".to_owned(),
                name: Some(context.source.node_text(name).to_owned()),
            })),
            None => found.push(Parameter {
                kind: child.kind_str().to_owned(),
                name: match child.kind_str() {
                    "identifier" => Some(context.source.node_text(child).to_owned()),
                    _ => child
                        .field("name")
                        .map(|name| context.source.node_text(name).to_owned()),
                },
            }),
        }
    }
    found
}

/// The names one `optional_parameter` node really stands for: the grammar folds a run of default
/// parameters into a chain of assignments, so the parameters after the first are buried inside the
/// value of the first.
fn unfolded_defaults<'tree>(parameter: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    if parameter.kind_str() != "optional_parameter" {
        return None;
    }
    let name = parameter.field("name")?;
    let mut current = parameter.field("value")?;
    folded_targets(current)?;

    let mut pending: std::collections::VecDeque<Node<'tree>> =
        std::collections::VecDeque::from([name]);
    let mut found: Vec<Node<'tree>> = Vec::new();
    loop {
        let Some(targets) = folded_targets(current) else {
            if let Some(name) = pending.pop_front() {
                found.push(name);
            }
            found.extend(pending);
            return Some(found);
        };
        let Some((_, names)) = targets.split_first() else {
            return Some(found);
        };
        if let Some(name) = pending.pop_front() {
            found.push(name);
        }
        pending.extend(names.iter().copied());
        current = current.field("right")?;
    }
}

/// The names an assignment written as a multiple assignment carries, which is how the fold shows.
fn folded_targets<'tree>(node: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    if node.kind_str() != "assignment" {
        return None;
    }
    let left = node.field("left")?;
    (left.kind_str() == "left_assignment_list").then(|| super::nodes::children(left))
}

/// `preprocess_super_args`: a brace-less hash written as the last argument is its pairs.
fn preprocess_super_args<'tree>(call: Node<'tree>) -> Vec<Node<'tree>> {
    arguments(call)
        .iter()
        .flat_map(|argument| argument.parts().to_vec())
        .collect()
}

/// `arguments_identical?`.
fn arguments_identical(
    context: &RuleContext<'_>,
    call: Node<'_>,
    definition: Node<'_>,
    parameters: &[Parameter],
    super_arguments: &[Node<'_>],
) -> bool {
    let block_forwarded = block_sends_to_super(call);
    // `argument_list_size_differs?`.
    let mut expected = parameters.len();
    if block_forwarded
        && parameters
            .iter()
            .any(|parameter| parameter.kind == "block_parameter")
    {
        expected -= 1;
    }
    if expected != super_arguments.len() {
        return false;
    }
    parameters
        .iter()
        .zip(super_arguments.iter())
        .all(|(parameter, argument)| {
            same_argument(context, definition, block_forwarded, parameter, *argument)
        })
}

fn same_argument(
    context: &RuleContext<'_>,
    definition: Node<'_>,
    block_forwarded: bool,
    parameter: &Parameter,
    argument: Node<'_>,
) -> bool {
    match parameter.kind.as_str() {
        // `positional_arg_same?`.
        "identifier" | "optional_parameter" => parameter
            .name
            .as_ref()
            .is_some_and(|name| names(argument, name, context)),
        // `positional_rest_arg_same?`.
        "splat_parameter" => match &parameter.name {
            None => is_anonymous_forward(argument, "splat_argument"),
            Some(name) => {
                argument.kind_str() == "splat_argument"
                    && argument
                        .named_child(0)
                        .is_some_and(|inner| names(inner, name, context))
            }
        },
        // `keyword_arg_same?`.
        "keyword_parameter" => {
            if argument.kind_str() != "pair" {
                return false;
            }
            let Some(key) = argument.field("key") else {
                return false;
            };
            match argument.field("value") {
                // `sym_node.source == lvar_node.source`: only the `name:` shorthand spelling.
                Some(value) => {
                    context.source.node_text(key) == context.source.node_text(value)
                        && symbol_name(key, context) == parameter.name.as_deref()
                        && value.kind_str() == "identifier"
                }
                // **`a:` with no value is the same pair spelled shorter.** The parser fills the
                // omitted value in with the variable of that name, so `super(a:)` passes `a` --
                // the grammar leaves the field empty instead.
                None => symbol_name(key, context) == parameter.name.as_deref(),
            }
        }
        // `keyword_rest_arg_same?`.
        "hash_splat_parameter" => match &parameter.name {
            None => is_anonymous_forward(argument, "hash_splat_argument"),
            Some(name) => {
                argument.kind_str() == "hash_splat_argument"
                    && argument
                        .named_child(0)
                        .is_some_and(|inner| names(inner, name, context))
            }
        },
        // `block_arg_same?`.
        "block_parameter" => {
            if block_forwarded {
                return true;
            }
            if argument.kind_str() != "block_argument" {
                return false;
            }
            match (&parameter.name, argument.named_child(0)) {
                (None, None) => true,
                (Some(name), Some(passed)) => {
                    names(passed, name, context) && !block_reassigned(context, definition, name)
                }
                _ => false,
            }
        }
        // `forward_arg_same?`.
        "forward_parameter" => argument.kind_str() == "forward_argument",
        _ => false,
    }
}

/// `forwarded_restarg` / `forwarded_kwrestarg`: `*` and `**` written with nothing after them.
fn is_anonymous_forward(argument: Node<'_>, kind: &str) -> bool {
    argument.kind_str() == kind && argument.named_child(0).is_none()
}

fn names(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "identifier" && context.source.node_text(node) == name
}

/// `block_sends_to_super?`: the block was written on the `super` itself, so it is what the block
/// parameter would have carried.
fn block_sends_to_super(call: Node<'_>) -> bool {
    call.field("block")
        .is_some_and(|block| BLOCKS.contains(&block.kind_str()))
}

/// `block_reassigned?`.
fn block_reassigned(context: &RuleContext<'_>, definition: Node<'_>, name: &str) -> bool {
    let mut stack = vec![definition];
    while let Some(node) = stack.pop() {
        if ASSIGN_TYPES.contains(&node.kind_str())
            && node
                .field("left")
                .is_some_and(|left| names(left, name, context))
        {
            return true;
        }
        crate::rules::push_named_children_in(node, context, &mut stack);
    }
    false
}
