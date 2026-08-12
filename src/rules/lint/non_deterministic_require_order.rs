use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, named_children, send_range};

use super::blocks::{BLOCK_KINDS, BlockArgs};
use super::locals::LocalVariables;

const MSG: &str = "Sort files before requiring them.";

/// `maximum_target_ruby_version 2.7`: `Dir.glob` sorts its results from Ruby 3.0 on.
const MAXIMUM_VERSION: RubyVersion = RubyVersion::new(2, 7);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() > MAXIMUM_VERSION {
        return;
    }
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        if let Some(block) = node
            .child_by_field_name("block")
            .filter(|block| BLOCK_KINDS.contains(&block.kind()))
        {
            check_block(node, block, context, &locals, offenses);
            continue;
        }
        check_block_pass(node, context, offenses);
    }
}

/// `on_block` and `on_numblock`: a loop over an unsorted `Dir` listing whose variable is required.
fn check_block(
    node: Node<'_>,
    block: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(body) = block.child_by_field_name("body") else {
        return;
    };
    if !unsorted_dir_loop(node, context) {
        return;
    }
    let names: Vec<String> = match BlockArgs::of(block, context, locals) {
        arguments if arguments.single_plain_arg() => {
            let BlockArgs::Written(parameters) = &arguments else {
                return;
            };
            vec![context.source.node_text(parameters[0]).to_owned()]
        }
        BlockArgs::Numbered(highest) => (1..=highest).map(|index| format!("_{index}")).collect(),
        _ => return,
    };
    if !names
        .into_iter()
        .any(|name| requires_variable(body, &name, context))
    {
        return;
    }
    let range = send_range(node, context);
    offenses.push(
        context
            .offense(MSG, range.clone())
            .corrected_by(correct_block(node, &range, context)),
    );
}

/// `on_block_pass`: `Dir.glob('*', &method(:require))` and the `each` form of it.
fn check_block_pass(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let given = arguments(node);
    let Some(last) = given.last() else {
        return;
    };
    if !is_method_require(last.first(), context) {
        return;
    }
    let glob_pass = unsorted_dir_block(node, context);
    if !glob_pass && !unsorted_dir_each(node, context) {
        return;
    }
    let range = node.byte_range();
    let edit = if glob_pass {
        // `Dir.glob('*', &method(:require))` keeps its block argument, moved behind `.sort.each`.
        // The Edits are emitted with the insertion first so that the engine, which treats every
        // zero-width edit as an insertion before its anchor, joins them the way upstream does.
        let block_argument = context.source.slice(last.range()).to_owned();
        let previous = given[given.len() - 2].range().end;
        vec![
            Edit {
                start: range.end,
                end: range.end,
                replacement: format!(".sort.each({block_argument})"),
                safe: true,
            },
            Edit {
                start: previous,
                end: last.range().end,
                replacement: String::new(),
                safe: true,
            },
        ]
    } else {
        let selector = node
            .child_by_field_name("method")
            .map_or(range.clone(), |method| method.byte_range());
        vec![Edit {
            start: selector.start,
            end: selector.end,
            replacement: "sort.each".to_owned(),
            safe: true,
        }]
    };
    offenses.push(context.offense(MSG, range).corrected_by_all(edit));
}

/// `correct_block`: the receiver of an `each` already lists the files, while a bare `Dir.glob` is
/// the listing itself.
fn correct_block(
    node: Node<'_>,
    range: &std::ops::Range<usize>,
    context: &RuleContext<'_>,
) -> Edit {
    let source = if unsorted_dir_block(node, context) {
        context.source.slice(range.clone()).to_owned()
    } else {
        node.child_by_field_name("receiver")
            .map_or_else(String::new, |receiver| {
                context.source.node_text(receiver).to_owned()
            })
    };
    Edit {
        start: range.start,
        end: range.end,
        replacement: format!("{source}.sort.each"),
        safe: true,
    }
}

fn unsorted_dir_loop(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    unsorted_dir_block(node, context) || unsorted_dir_each(node, context)
}

/// `(send (const {nil? cbase} :Dir) :glob ...)`.
fn unsorted_dir_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    is_dir_call(node, &["glob"], context)
}

/// `(send (send (const {nil? cbase} :Dir) {:[] :glob} ...) :each)`.
fn unsorted_dir_each(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !is_plain_send(node, context)
        || node
            .child_by_field_name("method")
            .is_none_or(|method| context.source.node_text(method) != "each")
    {
        return false;
    }
    let Some(receiver) = node.child_by_field_name("receiver") else {
        return false;
    };
    // `Dir['*']` is an `element_reference` here and a `send :[]` upstream.
    if receiver.kind() == "element_reference" {
        return receiver
            .child(0)
            .is_some_and(|target| is_top_level_dir(target, context));
    }
    is_dir_call(receiver, &["glob", "[]"], context)
}

fn is_dir_call(node: Node<'_>, methods: &[&str], context: &RuleContext<'_>) -> bool {
    node.kind() == "call"
        && is_plain_send(node, context)
        && node
            .child_by_field_name("method")
            .is_some_and(|method| methods.contains(&context.source.node_text(method)))
        && node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| is_top_level_dir(receiver, context))
}

fn is_top_level_dir(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    crate::rules::send_node::top_level_constant(node, "Dir", context)
}

/// `(block-pass (send nil? :method (sym {:require :require_relative})))`.
fn is_method_require(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind() != "block_argument" {
        return false;
    }
    let Some(call) = named_children(node).into_iter().next() else {
        return false;
    };
    if call.kind() != "call"
        || call.child_by_field_name("receiver").is_some()
        || call
            .child_by_field_name("method")
            .is_none_or(|method| context.source.node_text(method) != "method")
    {
        return false;
    }
    let given = arguments(call);
    given.len() == 1
        && crate::rules::send_node::symbol_name(given[0].first(), context)
            .is_some_and(|name| matches!(name, "require" | "require_relative"))
}

/// `(send nil? {:require :require_relative} (lvar %1))`, searched over the block body.
fn requires_variable(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    if node.kind() == "call"
        && node.child_by_field_name("receiver").is_none()
        && node.child_by_field_name("method").is_some_and(|method| {
            matches!(
                context.source.node_text(method),
                "require" | "require_relative"
            )
        })
    {
        let given = arguments(node);
        if given.len() == 1
            && given[0].first().kind() == "identifier"
            && context.source.node_text(given[0].first()) == name
        {
            return true;
        }
    }
    named_children(node)
        .into_iter()
        .any(|child| requires_variable(child, name, context))
}
