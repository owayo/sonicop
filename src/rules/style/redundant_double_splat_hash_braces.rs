//! `Style/RedundantDoubleSplatHashBraces`: `**{ a: 1 }` where `a: 1` says the same thing.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

const MSG: &str = "Remove the redundant double splat and braces, use keyword arguments directly.";

/// `MERGE_METHODS`.
const MERGE_METHODS: &[&str] = &["merge", "merge!"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("hash") {
        // `node.pairs` lists the `pair` children alone -- a nested `**{…}` is a `kwsplat` and is
        // not one of them. Counting it as a pair made every hash holding one look rocket-written
        // and dropped the **outer** hash of `**{a: 1, **{b: 2}}` on the floor.
        let written = super::nodes::children(node);
        let pairs: Vec<Node<'_>> = written
            .iter()
            .copied()
            .filter(|child| child.kind_str() == "pair")
            .collect();
        if pairs.is_empty() || pairs.iter().any(|pair| is_hash_rocket(*pair, context)) {
            continue;
        }
        let Some(parent) = node.parent() else {
            continue;
        };
        if !matches!(parent.kind_str(), "call" | "hash_splat_argument")
            || !mergeable(parent, context)
        {
            continue;
        }
        let Some(splat) = double_splat_ancestor(node) else {
            continue;
        };
        if allowed_double_splat_receiver(splat) {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, splat.byte_range())
                .corrected_by_all(autocorrect(context, node, splat, &written)),
        );
    }
}

/// `node.each_ancestor(:kwsplat).first`.
fn double_splat_ancestor<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind_str() == "hash_splat_argument" {
            return Some(ancestor);
        }
        current = ancestor.parent();
    }
    None
}

/// `allowed_double_splat_receiver?`.
fn allowed_double_splat_receiver(splat: Node<'_>) -> bool {
    let Some(first) = splat.named_child(0) else {
        return true;
    };
    if first.kind_str() == "call" && first.field("block").is_some() {
        return true;
    }
    if first.kind_str() != "call" {
        return false;
    }
    !root_receiver(first).is_some_and(|receiver| receiver.kind_str() == "hash")
}

/// `root_receiver`.
fn root_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let receiver = node.field("receiver")?;
    match receiver.field("receiver") {
        Some(_) => root_receiver(receiver),
        None => Some(receiver),
    }
}

/// `mergeable?`.
fn mergeable(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return true;
    }
    if !node
        .field("method")
        .is_some_and(|method| MERGE_METHODS.contains(&context.source.node_text(method)))
    {
        return false;
    }
    match node.parent() {
        Some(parent) => mergeable(parent, context),
        None => true,
    }
}

/// `autocorrect`: the `**` and the braces go, and every `merge` written after them becomes more
/// keyword arguments.
fn autocorrect(
    context: &RuleContext<'_>,
    node: Node<'_>,
    splat: Node<'_>,
    // Everything the braces hold, `kwsplat` children included: the braces come off around all of
    // it, not just around the pairs.
    pairs: &[Node<'_>],
) -> Vec<Edit> {
    let mut edits = Vec::new();
    // `kwsplat.loc.operator`: the `**` written in front.
    if let Some(first) = splat.named_child(0) {
        edits.push(remove(splat.start_byte()..first.start_byte()));
    }
    // `opening_brace` / `closing_brace`: the braces together with the space inside them.
    if let (Some(first), Some(last)) = (pairs.first(), pairs.last()) {
        edits.push(remove(node.start_byte()..first.start_byte()));
        edits.push(remove(last.end_byte()..node.end_byte()));
    }
    let merges = merge_calls(splat, context);
    if merges.is_empty() {
        return edits;
    }
    // `range_of_merge_methods`: from the innermost `.merge` through the end of the outermost.
    let (Some(inner), Some(outer)) = (merges.last(), merges.first()) else {
        return edits;
    };
    let Some(dot) = dot_position(*inner) else {
        return edits;
    };
    let mut moved: Vec<String> = merges
        .iter()
        .rev()
        .map(|call| {
            arguments(*call)
                .iter()
                .map(|argument| match argument.parts() {
                    [single] if single.kind_str() == "hash" => {
                        context.source.node_text(*single).to_owned()
                    }
                    parts => parts
                        .iter()
                        .map(|part| match part.kind_str() {
                            "pair" | "hash_splat_argument" => {
                                context.source.node_text(*part).to_owned()
                            }
                            _ => format!("**{}", context.source.node_text(*part)),
                        })
                        .collect::<Vec<String>>()
                        .join(", "),
                })
                .collect::<Vec<String>>()
                .join(", ")
        })
        .collect();
    moved.insert(0, String::new());
    edits.push(Edit {
        start: dot,
        end: outer.end_byte(),
        replacement: moved.join(", "),
        safe: true,
    });
    edits
}

/// `select_merge_method_nodes`, in the order `each_descendant` walks them.
fn merge_calls<'tree>(splat: Node<'tree>, context: &RuleContext<'_>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let mut stack: Vec<Node<'tree>> = Vec::new();
    crate::rules::push_named_children(splat, &mut stack);
    while let Some(node) = stack.pop() {
        if node.kind_str() == "call" && mergeable(node, context) {
            found.push(node);
        }
        crate::rules::push_named_children(node, &mut stack);
    }
    found.sort_by_key(Node::start_byte);
    found
}

/// `node.loc.dot.begin`.
fn dot_position(call: Node<'_>) -> Option<usize> {
    let receiver = call.field("receiver")?;
    let method = call.field("method")?;
    (receiver.end_byte() < method.start_byte()).then_some(receiver.end_byte())
}

/// `(pair _ _)` written with `=>` rather than with a `key:`.
fn is_hash_rocket(pair: Node<'_>, context: &RuleContext<'_>) -> bool {
    pair.kind_str() != "pair"
        || pair
            .child(1)
            .is_some_and(|separator| context.source.node_text(separator) == "=>")
}

fn remove(range: std::ops::Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}
