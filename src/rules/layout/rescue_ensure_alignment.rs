//! `Layout/RescueEnsureAlignment`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::{character_column, parser_node_start, start_line_range};
use crate::rules::node_ext::NodeExt;

/// `ANCESTOR_TYPES`, as the grammar spells them.
const ANCESTOR_KINDS: [&str; 8] = [
    "begin",
    "method",
    "singleton_method",
    "class",
    "module",
    "singleton_class",
    "block",
    "do_block",
];

const ACCESS_MODIFIERS: [&str; 6] = [
    "public",
    "protected",
    "private",
    "module_function",
    "public_class_method",
    "private_class_method",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // A `rescue` modifier is its own node in the grammar, which is what `modifier?` filters out.
    for node in context.nodes_of_any(&["rescue", "ensure"]) {
        let Some(keyword) = node.child(0).filter(|child| !child.is_named()) else {
            continue;
        };
        let Some(alignment) = alignment_node(context, node, keyword) else {
            continue;
        };
        let location = alignment_location(context, alignment);
        let keyword_line = context.source.line_column(keyword.start_byte()).0;
        let keyword_column = character_column(context, keyword.start_byte());
        let alignment_line = context.source.line_column(location.start).0;
        let alignment_column = character_column(context, location.start);
        if alignment_column == keyword_column || alignment_line == keyword_line {
            continue;
        }

        let beginning = beginning(context, alignment, location.start);
        let message = format!(
            "`{}` at {keyword_line}, {keyword_column} is not aligned with `{beginning}` at \
             {alignment_line}, {alignment_column}.",
            context.source.node_text(keyword)
        );
        let mut offense = context.offense(message, keyword.byte_range());
        // `autocorrect` gives up when something else already sits on the keyword's line.
        let whitespace = context.source.line_start(keyword_line)..keyword.start_byte();
        if context.source.text()[whitespace.clone()].trim().is_empty() {
            offense = offense.corrected_by(Edit {
                start: whitespace.start,
                end: whitespace.end,
                replacement: " ".repeat(usize::try_from(alignment_column).unwrap_or(0)),
                safe: true,
            });
        }
        offenses.push(offense);
    }
}

/// `alignment_node`: the construct the keyword belongs to, once the wrappers upstream prefers to
/// align against have been taken into account.
fn alignment_node<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    keyword: Node<'_>,
) -> Option<Node<'tree>> {
    let ancestor = ancestor(node)?;
    if ancestor.kind_str() == "begin" {
        return Some(ancestor);
    }
    if matches!(ancestor.kind_str(), "block" | "do_block")
        && aligned_with_line_break_method(context, ancestor, keyword)
    {
        return None;
    }
    // An assignment written on the ancestor's own line takes its place.
    let parent = parser_parent(ancestor);
    if let Some(assignment) = parent.filter(|parent| {
        matches!(parent.kind_str(), "assignment" | "operator_assignment")
            && context.source.line_column(parent.start_byte()).0
                == context.source.line_column(parser_node_start(ancestor)).0
    }) {
        return Some(assignment);
    }
    if matches!(ancestor.kind_str(), "method" | "singleton_method") {
        if let Some(modifier) = parent.filter(|parent| is_access_modifier(context, *parent)) {
            return Some(modifier);
        }
    }
    Some(ancestor)
}

/// `node.ancestors.first` as upstream's tree has it. A block literal is one node there spanning the
/// call it hangs off, and an argument is a direct child of the call it was written in, so both of
/// the nodes the grammar puts in between are stepped over.
fn parser_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = match node.kind_str() {
        "block" | "do_block" => node.parent()?.parent(),
        _ => node.parent(),
    }?;
    match parent.kind_str() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}

fn ancestor<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if ANCESTOR_KINDS.contains(&candidate.kind_str()) {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// `aligned_with_line_break_method?`: a block opened on the last line of a chained call lets the
/// keyword line up with that call's dot or selector instead.
fn aligned_with_line_break_method(
    context: &RuleContext<'_>,
    block: Node<'_>,
    keyword: Node<'_>,
) -> bool {
    let Some(send) = block.parent_of(context) else {
        return false;
    };
    let Some(open) = block_open(block) else {
        return false;
    };
    let open_line = context.source.line_column(open.start_byte()).0;
    let keyword_column = character_column(context, keyword.start_byte());
    if let Some(dot) = send.field("operator") {
        if context.source.line_column(dot.start_byte()).0 == open_line
            && character_column(context, dot.start_byte()) == keyword_column
        {
            return true;
        }
    }
    let selector = send
        .field("method")
        .filter(|method| !method.byte_range().is_empty())
        .unwrap_or(send);
    context.source.line_column(selector.start_byte()).0 == open_line
        && character_column(context, selector.start_byte()) == keyword_column
}

/// `alignment_location` under the default `start_of_line` style of `Layout/BeginEndAlignment`.
fn alignment_location(context: &RuleContext<'_>, alignment: Node<'_>) -> Range<usize> {
    start_line_range(context, parser_node_start(alignment))
}

/// `alignment_source`: from the start of the alignment line to the end of whatever names the
/// construct.
fn beginning(context: &RuleContext<'_>, alignment: Node<'_>, start: usize) -> String {
    let end = ending(alignment).unwrap_or_else(|| alignment.end_byte());
    let end = end.max(start);
    context.source.text()[start..end].to_owned()
}

fn ending(node: Node<'_>) -> Option<usize> {
    match node.kind_str() {
        "block" | "do_block" => block_open(node).map(|open| open.end_byte()),
        "begin" => child_of_kind(node, "begin").map(|keyword| keyword.end_byte()),
        "method" | "singleton_method" | "class" | "module" => {
            node.field("name").map(|name| name.end_byte())
        }
        "singleton_class" => node.field("value").map(|value| value.end_byte()),
        "assignment" | "operator_assignment" => node.field("left").map(|left| {
            // `obj.attr = …` is a `send` of `:attr=` upstream, which falls to the wrapper branch
            // and aligns against the **receiver**; only a plain variable aligns against its name.
            match left.kind_str() {
                "call" => left
                    .field("receiver")
                    .map_or_else(|| left.end_byte(), |receiver| receiver.end_byte()),
                _ => left.end_byte(),
            }
        }),
        // A wrapper such as an access modifier: its receiver, or the name of what it wraps.
        _ => node
            .field("receiver")
            .map(|receiver| receiver.end_byte())
            .or_else(|| wrapped_name_end(node)),
    }
}

fn wrapped_name_end(node: Node<'_>) -> Option<usize> {
    // `node.child_nodes.first` for a `send` is its **first argument**: the method name reaches
    // upstream as a symbol, not a node. The grammar makes it an `identifier` child, and taking that
    // as the first child asked it for a `name` field it does not have -- the whole call then went
    // into the message instead of just `private_class_method def test`.
    let selector = node.field("method").map(|method| method.id());
    let mut cursor = node.walk();
    let child = node.named_children(&mut cursor).find(|child| {
        !matches!(child.kind_str(), "comment" | "heredoc_body") && Some(child.id()) != selector
    })?;
    let inner = if child.kind_str() == "argument_list" {
        let mut inner_cursor = child.walk();
        child
            .named_children(&mut inner_cursor)
            .find(|node| !matches!(node.kind_str(), "comment" | "heredoc_body"))?
    } else {
        child
    };
    inner.field("name").map(|name| name.end_byte())
}

/// `access_modifier?`: `private def foo` and the class-method variants.
fn is_access_modifier(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    node.field("method")
        .is_some_and(|method| ACCESS_MODIFIERS.contains(&context.source.node_text(method)))
}

fn block_open<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| matches!(child.kind_str(), "{" | "do"))
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == kind)
}
