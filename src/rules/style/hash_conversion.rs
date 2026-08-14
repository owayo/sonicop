//! `Style/HashConversion`: `Hash[...]` is a hash literal or a `to_h` written the long way.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

const MSG_TO_H: &str = "Prefer `ary.to_h` to `Hash[ary]`.";
const MSG_LITERAL_MULTI_ARG: &str = "Prefer literal hash to `Hash[arg1, arg2, ...]`.";
const MSG_LITERAL_HASH_ARG: &str = "Prefer literal hash to `Hash[key: value, ...]`.";
const MSG_SPLAT: &str = "Prefer `array_of_pairs.to_h` to `Hash[*array]`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_splat = context
        .setting::<bool>("AllowSplatArgument")
        .unwrap_or(true);
    // `ignore_node`: a `Hash[...]` inside another one is left to the pass that has rewritten the
    // outer. The walk reaches the outer first.
    let mut ignored: HashSet<usize> = HashSet::new();
    for node in context.nodes_of("element_reference") {
        if !is_hash_index(node, context) || has_ignored_ancestor(node, context, &ignored) {
            continue;
        }
        let list = index_arguments(node);
        match list.as_slice() {
            [only] => single_argument(context, node, only, allow_splat, offenses),
            several => multi_argument(context, node, several, offenses),
        }
        ignored.insert(node.id());
    }
}

/// The four shapes a one-argument `Hash[...]` can take.
fn single_argument(
    context: &RuleContext<'_>,
    node: Node<'_>,
    argument: &[Node<'_>],
    allow_splat: bool,
    offenses: &mut Vec<Offense>,
) {
    let first = argument[0];
    // A trailing run of pairs is the `hash` upstream's parser builds, and a braced literal is one
    // already. Upstream wraps the argument's *source* either way, so a braced one gains a second
    // pair of braces -- which is what it does, however odd the result reads.
    if argument.len() > 1
        || matches!(first.kind_str(), "pair" | "hash_splat_argument" | "hash")
    {
        let inner = first.start_byte()..argument[argument.len() - 1].end_byte();
        let mut edits = vec![Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: format!("{{{}}}", context.source.slice(inner)),
            safe: true,
        }];
        edits.extend(parenthesize_parent(context, node, false));
        offenses.push(
            context
                .offense(MSG_LITERAL_HASH_ARG, node.byte_range())
                .corrected_by_all(edits),
        );
        return;
    }
    if first.kind_str() == "splat_argument" || first.kind_str() == "forward_argument" {
        if !allow_splat {
            offenses.push(context.offense(MSG_SPLAT, node.byte_range()));
        }
        return;
    }
    // `use_zip_method_without_argument?`: `Hash[a.zip]` needs the second array spelled out.
    if let Some(zip) = zip_without_argument(first, context) {
        let edit = match closing_paren(zip) {
            Some(close) => Edit {
                start: close,
                end: close,
                replacement: "[]".to_owned(),
                safe: true,
            },
            None => Edit {
                start: zip.end_byte(),
                end: zip.end_byte(),
                replacement: "([])".to_owned(),
                safe: true,
            },
        };
        offenses.push(
            context
                .offense(MSG_TO_H, node.byte_range())
                .corrections_anchored_at(zip.byte_range())
                .corrected_by(edit),
        );
        return;
    }
    let source = context.source.node_text(first);
    let replacement = if requires_parens(first, context) {
        format!("({source})")
    } else {
        source.to_owned()
    };
    offenses.push(
        context
            .offense(MSG_TO_H, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!("{replacement}.to_h"),
                safe: true,
            }),
    );
}

/// `Hash[a, b, ...]`, which is a hash literal when the count is even.
fn multi_argument(
    context: &RuleContext<'_>,
    node: Node<'_>,
    list: &[Vec<Node<'_>>],
    offenses: &mut Vec<Offense>,
) {
    if list
        .iter()
        .any(|argument| argument[0].kind_str() == "splat_argument")
    {
        return;
    }
    if list.len() % 2 == 1 {
        offenses.push(context.offense(MSG_LITERAL_MULTI_ARG, node.byte_range()));
        return;
    }
    let content = list
        .chunks(2)
        .map(|pair| {
            format!(
                "{} => {}",
                context.source.node_text(pair[0][0]),
                context.source.node_text(pair[1][0])
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut edits = vec![Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: format!("{{{content}}}"),
        safe: true,
    }];
    edits.extend(parenthesize_parent(context, node, true));
    offenses.push(
        context
            .offense(MSG_LITERAL_MULTI_ARG, node.byte_range())
            .corrected_by_all(edits),
    );
}

/// `add_parentheses(parent, corrector)`: a hash literal handed to a call without parentheses would
/// read as a block, so the call gets them.
fn parenthesize_parent(context: &RuleContext<'_>, node: Node<'_>, skip_to_h: bool) -> Vec<Edit> {
    let Some(parent) = context.parent(node).and_then(enclosing_call) else {
        return Vec::new();
    };
    if parent.kind_str() != "call" {
        return Vec::new();
    }
    let Some(selector) = parent.field("method") else {
        return Vec::new();
    };
    if skip_to_h && context.source.node_text(selector) == "to_h" {
        return Vec::new();
    }
    if closing_paren(parent).is_some() {
        return Vec::new();
    }
    if arguments(parent).is_empty() {
        return vec![Edit {
            start: parent.end_byte(),
            end: parent.end_byte(),
            replacement: "()".to_owned(),
            safe: true,
        }];
    }
    // `args_begin` is the one character after the selector, which `remove` and `insert_before`
    // together turn into the opening parenthesis.
    let start = selector.end_byte();
    vec![
        Edit {
            start,
            end: start + 1,
            replacement: "(".to_owned(),
            safe: true,
        },
        Edit {
            start: parent.end_byte(),
            end: parent.end_byte(),
            replacement: ")".to_owned(),
            safe: true,
        },
    ]
}

/// `requires_parens?`.
fn requires_parens(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "call" {
        if node
            .field("method")
            .is_some_and(|name| context.source.node_text(name) == "[]")
        {
            return false;
        }
        if !arguments(node).is_empty() && closing_paren(node).is_none() {
            return true;
        }
        return false;
    }
    if node.kind_str() == "element_reference" {
        return false;
    }
    // `operator_keyword?`: `and` and `or`.
    node.kind_str() == "binary"
        && node.field("operator").is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "&&" | "||" | "and" | "or"
            )
        })
}

/// `(send _ :zip)` with no arguments.
fn zip_without_argument<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if node.kind_str() != "call" || node.field("block").is_some() {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "zip" {
        return None;
    }
    arguments(node).is_empty().then_some(node)
}

/// `(const {nil? cbase} :Hash)` as the object being indexed.
fn is_hash_index(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("object")
        .is_some_and(|object| super::nodes::is_top_level_constant(object, "Hash", context))
}

/// The index arguments, grouped the way upstream's parser folds a trailing run of pairs into one
/// `hash`.
fn index_arguments<'tree>(node: Node<'tree>) -> Vec<Vec<Node<'tree>>> {
    let object = node.field("object").map(|object| object.id());
    let mut grouped: Vec<Vec<Node<'tree>>> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for child in super::nodes::children(node) {
        if Some(child.id()) == object {
            continue;
        }
        if matches!(child.kind_str(), "pair" | "hash_splat_argument") {
            hash.push(child);
            continue;
        }
        if !hash.is_empty() {
            grouped.push(std::mem::take(&mut hash));
        }
        grouped.push(vec![child]);
    }
    if !hash.is_empty() {
        grouped.push(hash);
    }
    grouped
}

/// The call a node is the receiver or an argument of, seen through the argument list.
fn enclosing_call<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind_str() == "argument_list" {
        return node.parent();
    }
    Some(node)
}

/// The `)` that closes a call's argument list, if it was written with one.
fn closing_paren(node: Node<'_>) -> Option<usize> {
    let list = node.field("arguments")?;
    let last = list.child(list.child_count().checked_sub(1)? as u32)?;
    (last.kind_str() == ")").then(|| last.start_byte())
}

/// Whether any enclosing `Hash[...]` has already been folded.
fn has_ignored_ancestor(
    node: Node<'_>,
    context: &RuleContext<'_>,
    ignored: &HashSet<usize>,
) -> bool {
    let mut current = context.parent(node);
    while let Some(parent) = current {
        if ignored.contains(&parent.id()) {
            return true;
        }
        current = context.parent(parent);
    }
    false
}
