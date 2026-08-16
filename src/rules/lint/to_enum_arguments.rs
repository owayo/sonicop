use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments, named_children, pair_key_symbol, symbol_name};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        // `on_send` は `csend` に呼ばれない。`alias on_csend on_send` を書いていない cop は
        // `x&.foo` を構造的に一切見ないので、ここで落とさないと過剰検出になる。
        if !crate::rules::send_node::is_plain_send(node, context) {
            continue;
        }
        if !matches!(context.source.node_text(method), "to_enum" | "enum_for") {
            continue;
        }
        // `(send {nil? self} …)`.
        if node
            .field("receiver")
            .is_some_and(|receiver| receiver.kind_str() != "self")
        {
            continue;
        }
        let Some(definition) = enclosing_definition(node, context) else {
            continue;
        };
        let call_arguments = arguments(node);
        let Some((name, rest)) = call_arguments.split_first() else {
            continue;
        };
        if !names_this_method(name.first(), definition, context) {
            continue;
        }
        if arguments_match(rest, definition, context) {
            continue;
        }
        // Upstream's `send` ends where its arguments do -- a block written after it belongs to the
        // `block` node wrapped around the call, and the reported range stops before it.
        offenses.push(context.offense(
            "Ensure you correctly provided all the arguments.",
            crate::rules::send_node::send_range(node, context),
        ));
    }
}

fn enclosing_definition<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "method" | "singleton_method") {
            return Some(ancestor);
        }
        current = ancestor.parent_of(context);
    }
    None
}

/// `method_name?`: `__method__`, `__callee__`, or the name spelled out as a symbol.
fn names_this_method(node: Node<'_>, definition: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(name) = definition
        .field("name")
        .map(|name| context.source.node_text(name))
    else {
        return false;
    };
    match node.kind_str() {
        "identifier" => matches!(context.source.node_text(node), "__method__" | "__callee__"),
        _ => symbol_name(node, context) == Some(name),
    }
}

/// `arguments_match?`: every parameter is passed on, and nothing else is.
fn arguments_match(
    passed: &[Argument<'_>],
    definition: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    let parameters = definition_parameters(definition);
    let mut index = 0;
    for parameter in &parameters {
        if parameter.kind_str() == "block_parameter" {
            continue;
        }
        let argument = passed.get(index);
        if matches!(
            parameter.kind_str(),
            "identifier" | "splat_parameter" | "optional_parameter" | "destructured_parameter"
        ) {
            index += 1;
        }
        let Some(argument) = argument else {
            return false;
        };
        if !argument_match(argument, *parameter, context) {
            return false;
        }
    }
    !extra_positional_arguments(passed, &parameters, context)
        && !extra_keyword_arguments(passed, &parameters, context)
}

/// `argument_match?`.
fn argument_match(argument: &Argument<'_>, parameter: Node<'_>, context: &RuleContext<'_>) -> bool {
    let source = context.source.slice(argument.range());
    match parameter.kind_str() {
        // `arg` and `restarg` are compared by how they were written.
        "identifier" | "splat_parameter" | "destructured_parameter" => {
            source == context.source.node_text(parameter)
        }
        // `optarg` is compared by name alone, since the default is not passed on.
        "optional_parameter" => parameter
            .field("name")
            .is_some_and(|name| source == context.source.node_text(name)),
        "keyword_parameter" => {
            let Some(name) = parameter
                .field("name")
                .map(|name| context.source.node_text(name))
            else {
                return false;
            };
            is_keyword_hash(argument)
                && argument
                    .parts()
                    .iter()
                    .any(|pair| passes_keyword(*pair, name, context))
        }
        "hash_splat_parameter" => {
            is_keyword_hash(argument)
                && argument.parts().iter().any(|part| {
                    part.kind_str() == "hash_splat_argument"
                        && context.source.node_text(*part) == context.source.node_text(parameter)
                })
        }
        "forward_parameter" => context.source.slice(argument.range()) == "...",
        _ => false,
    }
}

/// `passing_keyword_arg?`: `(pair (sym name) (lvar name))`.
fn passes_keyword(pair: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    pair_key_symbol(pair, context) == Some(name)
        && pair.field("value").is_some_and(|value| {
            value.kind_str() == "identifier" && context.source.node_text(value) == name
        })
}

/// `keyword_hash_argument?`: the run of pairs upstream folds into a brace-less `hash`.
fn is_keyword_hash(argument: &Argument<'_>) -> bool {
    argument
        .parts()
        .iter()
        .all(|part| matches!(part.kind_str(), "pair" | "hash_splat_argument"))
}

fn extra_positional_arguments(
    passed: &[Argument<'_>],
    parameters: &[Node<'_>],
    context: &RuleContext<'_>,
) -> bool {
    // `variadic_parameters?` and `expandable_arguments?`: either side may stand for any number.
    if parameters.iter().any(|parameter| {
        matches!(
            parameter.kind_str(),
            "splat_parameter" | "forward_parameter"
        )
    }) || passed.iter().any(|argument| {
        matches!(
            argument.first().kind_str(),
            "splat_argument" | "forward_argument"
        ) || context.source.slice(argument.range()) == "..."
    }) {
        return false;
    }
    let positional = passed
        .iter()
        .filter(|argument| !is_keyword_hash(argument) && argument.first().kind_str() != "hash")
        .filter(|argument| argument.first().kind_str() != "block_argument")
        .count();
    let declared = parameters
        .iter()
        .filter(|parameter| {
            matches!(
                parameter.kind_str(),
                "identifier" | "optional_parameter" | "destructured_parameter"
            )
        })
        .count();
    positional > declared
}

fn extra_keyword_arguments(
    passed: &[Argument<'_>],
    parameters: &[Node<'_>],
    context: &RuleContext<'_>,
) -> bool {
    if parameters.iter().any(|parameter| {
        matches!(
            parameter.kind_str(),
            "hash_splat_parameter" | "forward_parameter"
        )
    }) {
        return false;
    }
    if passed.iter().any(|argument| {
        context.source.slice(argument.range()) == "..."
            || argument
                .parts()
                .iter()
                .any(|part| part.kind_str() == "hash_splat_argument")
    }) {
        return false;
    }
    let declared: Vec<&str> = parameters
        .iter()
        .filter(|parameter| parameter.kind_str() == "keyword_parameter")
        .filter_map(|parameter| {
            parameter
                .field("name")
                .map(|name| context.source.node_text(name))
        })
        .collect();
    passed.iter().any(|argument| {
        is_keyword_hash(argument)
            && argument.parts().iter().any(|pair| {
                pair_key_symbol(*pair, context).is_some_and(|passed| !declared.contains(&passed))
            })
    })
}

fn definition_parameters<'tree>(definition: Node<'tree>) -> Vec<Node<'tree>> {
    definition
        .field("parameters")
        .map(|parameters| {
            named_children(parameters)
                .into_iter()
                .filter(|child| child.kind_str() != "comment")
                .collect()
        })
        .unwrap_or_default()
}
