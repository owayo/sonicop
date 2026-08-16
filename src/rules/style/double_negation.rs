use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::is_plain_send;

const MSG: &str = "Avoid the use of double negation (`!!`).";

/// `Node#conditional?`: `CONDITIONALS`, plus the modifier and ternary spellings the grammar keeps
/// apart and upstream's parser does not.
const CONDITIONALS: &[&str] = &[
    "if",
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

/// Wrappers the grammar puts between a call and its arguments or a clause and its statements.
const LISTS: &[&str] = &["argument_list", "block_body", "do", "body_statement"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed_in_returns = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "allowed_in_returns".to_owned())
        == "allowed_in_returns";

    // `(send (send _ :!) :!)`: **both levels are a plain `send`.** The grammar spells one `!` as a
    // `unary` and the other as a `call`, so walking only `unary` loses `foo.!.!` entirely, and
    // walking `call` without asking for a plain send reports `!foo&.!`, which is a `csend` upstream
    // and matches nothing.
    let nodes: Vec<Node<'_>> = context
        .nodes_of("unary")
        .chain(context.nodes_of("call"))
        .collect();
    for node in nodes {
        let Some(selector) = bang(context, node) else {
            continue;
        };
        let operand = match node.kind_str() {
            "unary" => node.field("operand"),
            _ => node.field("receiver"),
        };
        if !operand.is_some_and(|operand| is_negation(context, operand)) {
            continue;
        }
        if allowed_in_returns && allowed_in_returns_here(context, node) {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, selector.byte_range())
                .corrected_by_all([
                    Edit {
                        start: selector.start_byte(),
                        end: selector.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: node.end_byte(),
                        end: node.end_byte(),
                        replacement: ".nil?".to_owned(),
                        safe: true,
                    },
                ])
                // `insert_after(node, '.nil?')` hangs off the whole expression, while the offense
                // is reported on the `!` alone.
                .corrections_anchored_at(node.byte_range()),
        );
    }
}

/// `node.prefix_bang?`: the `!` this cop reports on, whichever way it was written.
///
/// `not not x` is the same `(send (send _ :!) :!)` upstream, but `loc.selector` is `not`, so the
/// outer one has to be spelled `!`. A `&.!` is a `csend`, which the pattern does not match at all.
fn bang<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    let selector = match node.kind_str() {
        "unary" => node.field("operator")?,
        "call" => node
            .field("method")
            .filter(|_| is_plain_send(node, context))?,
        _ => return None,
    };
    (context.source.node_text(selector) == "!").then_some(selector)
}

/// `(send _ :!)`, which `not x` and `x.!` are as much as `!x` -- but `x&.!` is not.
fn is_negation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "unary" => node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not")),
        "call" => {
            is_plain_send(node, context)
                && node
                    .field("method")
                    .is_some_and(|method| context.source.node_text(method) == "!")
        }
        _ => false,
    }
}

/// `allowed_in_returns?`: the value the enclosing method hands back may be spelled this way.
fn allowed_in_returns_here(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if upstream_parent(node).is_some_and(|parent| parent.kind_str() == "return") {
        return true;
    }
    end_of_method_definition(context, node)
}

fn end_of_method_definition(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(definition) = definition_from_ascendant(context, node) else {
        return false;
    };
    let Some(last_child) = definition_last_child(definition) else {
        return false;
    };
    match conditional_from_ascendant(node) {
        Some(conditional) => condition_return_value(node, last_child, conditional),
        None => {
            if matches!(last_child.kind_str(), "pair" | "hash")
                || last_child
                    .parent_of(context)
                    .is_some_and(|parent| parent.kind_str() == "array")
            {
                return false;
            }
            last_child.start_position().row <= node.start_position().row
        }
    }
}

/// What `find_def_node_from_ascendant` settles on: the `def` the expression is written in, or the
/// `define_method` call standing in for one.
fn definition_from_ascendant<'t>(
    context: &RuleContext<'_>,
    node: Node<'t>,
) -> Option<Definition<'t>> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if matches!(parent.kind_str(), "method" | "singleton_method") {
            return Some(Definition::Method(parent.field("body")?));
        }
        if let Some(call) = define_method_call(context, parent) {
            return Some(Definition::DefineMethod(call));
        }
        current = parent;
    }
    None
}

enum Definition<'t> {
    /// The body list of a `def`, which is what `find_last_child` is handed.
    Method(Node<'t>),
    /// The `define_method(:name)` call, which stands in for the definition itself.
    DefineMethod(Node<'t>),
}

/// `define_method?`: a block whose call is `define_method` or `define_singleton_method`.
fn define_method_call<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<Node<'t>> {
    if !matches!(node.kind_str(), "block" | "do_block") {
        return None;
    }
    let call = node.parent().filter(|call| call.kind_str() == "call")?;
    let method = call.field("method")?;
    matches!(
        context.source.node_text(method),
        "define_method" | "define_singleton_method"
    )
    .then_some(call)
}

/// `find_last_child(def_node.send_type? ? def_node : def_node.body)`.
fn definition_last_child<'t>(definition: Definition<'t>) -> Option<Node<'t>> {
    match definition {
        // The `define_method(...)` send stands alone upstream: the block hangs off it rather than
        // being part of it, so its last child is its last argument.
        Definition::DefineMethod(call) => call
            .field("arguments")
            .map(super::nodes::children)
            .and_then(|arguments| arguments.last().copied())
            .or_else(|| call.field("receiver")),
        Definition::Method(body) => {
            // A body split by a `rescue` or an `ensure` is that node upstream, and
            // `find_last_child` walks straight through it to the statements it guards.
            let statements: Vec<Node<'t>> = super::nodes::children(body)
                .into_iter()
                .filter(|child| !matches!(child.kind_str(), "rescue" | "ensure" | "else"))
                .collect();
            match statements.as_slice() {
                [] => None,
                // A lone parenthesized statement is a `begin` holding it.
                [only] if only.kind_str() == "parenthesized_statements" => {
                    super::nodes::children(*only).last().copied()
                }
                [only] => child_nodes(*only).last().copied(),
                several => several.last().copied(),
            }
        }
    }
}

/// `node.child_nodes`, which the grammar's argument and statement lists have no counterpart for.
fn child_nodes<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    if matches!(node.kind_str(), "call" | "method_call")
        && let Some(block) = node.field("block")
    {
        // A call carrying a block is a `block` node upstream, whose last child is the block body.
        return super::nodes::children(block);
    }
    let method = node
        .field("method")
        .filter(|_| matches!(node.kind_str(), "call" | "method_call"));
    let mut children = Vec::new();
    for child in super::nodes::children(node) {
        if method.is_some_and(|method| method.id() == child.id()) {
            continue;
        }
        match LISTS.contains(&child.kind_str()) {
            true => children.extend(super::nodes::children(child)),
            false => children.push(child),
        }
    }
    children
}

/// `find_conditional_node_from_ascendant`.
fn conditional_from_ascendant<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if CONDITIONALS.contains(&parent.kind_str()) {
            return Some(parent);
        }
        current = parent;
    }
    None
}

/// `double_negative_condition_return_value?`.
fn condition_return_value(node: Node<'_>, last_child: Node<'_>, conditional: Node<'_>) -> bool {
    match parent_not_enumerable(node) {
        Some(parent) if is_begin(parent) => node.start_position().row == parent.end_position().row,
        _ => last_child.end_position().row <= conditional.end_position().row,
    }
}

/// `find_parent_not_enumerable`, with the grammar's statement lists read as the `begin` upstream
/// builds for more than one statement.
fn parent_not_enumerable<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut current = node.parent()?;
    while matches!(current.kind_str(), "pair" | "hash" | "array") {
        current = current.parent()?;
    }
    Some(current)
}

fn is_begin(node: Node<'_>) -> bool {
    match node.kind_str() {
        "parenthesized_statements" => true,
        "then" | "else" | "body_statement" | "block_body" | "do" | "program" => {
            super::nodes::children(node).len() > 1
        }
        _ => false,
    }
}

/// The node upstream would call the parent: an argument list is no node of its own there.
fn upstream_parent<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let parent = node.parent()?;
    match parent.kind_str() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}
