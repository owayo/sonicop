use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `minimum_target_ruby_version 3.2`: from then on a `Struct` accepts keyword arguments anyway.
const MINIMUM: RubyVersion = RubyVersion::new(3, 2);

/// `Struct.new(..., keyword_init: true)`, whose option no longer changes anything.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of("call") {
        let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method")) else {
            continue;
        };
        if context.source.node_text(selector) != "new"
            || !send_node::top_level_constant(receiver, "Struct", context)
        {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let Some((pairs, preceding)) = trailing_hash(&arguments) else {
            continue;
        };
        let options: Vec<(Node<'_>, bool)> = pairs
            .iter()
            .filter_map(|pair| keyword_init(*pair, context).map(|redundant| (*pair, redundant)))
            .collect();
        // A `keyword_init: false` still means something, so nothing in the call is reported.
        if options.iter().any(|(_, redundant)| !redundant) {
            continue;
        }
        let all_are_options = options.len() == pairs.len();
        for (pair, _) in &options {
            let value = pair.field("value").map_or("", |node| context.source.node_text(node));
            let range = removal(*pair, &pairs, all_are_options, preceding);
            offenses.push(
                context
                    .offense(
                        format!("Remove the redundant `keyword_init: {value}`."),
                        pair.byte_range(),
                    )
                    .corrected_by(Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
    }
}

/// The pairs of the trailing hash argument, and the argument written in front of it.
///
/// Upstream's parser wraps a run of trailing keyword arguments in a `hash` node; the grammar leaves
/// them loose in the argument list, so the run is put back together here. Braces written out give a
/// `hash` node in both.
fn trailing_hash<'tree>(
    arguments: &[Node<'tree>],
) -> Option<(Vec<Node<'tree>>, Option<Node<'tree>>)> {
    let last = arguments.last()?;
    if last.kind_str() == "hash" {
        let pairs = super::nodes::children(*last)
            .into_iter()
            .filter(|child| child.kind_str() == "pair")
            .collect();
        return Some((pairs, arguments.len().checked_sub(2).map(|index| arguments[index])));
    }
    if !matches!(last.kind_str(), "pair" | "hash_splat_argument") {
        return None;
    }
    let start = arguments
        .iter()
        .rposition(|argument| !matches!(argument.kind_str(), "pair" | "hash_splat_argument"))
        .map_or(0, |index| index + 1);
    let pairs = arguments[start..]
        .iter()
        .copied()
        .filter(|child| child.kind_str() == "pair")
        .collect();
    Some((pairs, start.checked_sub(1).map(|index| arguments[index])))
}

/// `keyword_init?`, answering `redundant_keyword_init?` at the same time: `Some(true)` for `true`
/// and `nil`, `Some(false)` for `false`, `None` for any other pair.
fn keyword_init(pair: Node<'_>, context: &RuleContext<'_>) -> Option<bool> {
    if pair.kind_str() != "pair" {
        return None;
    }
    let key = pair.field("key")?;
    if send_node::symbol_name(key, context) != Some("keyword_init") {
        return None;
    }
    match pair.field("value")?.kind_str() {
        "true" | "nil" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// `range(redundant_keyword_init)`: what to take out so the call still reads.
fn removal(
    pair: Node<'_>,
    pairs: &[Node<'_>],
    all_are_options: bool,
    preceding: Option<Node<'_>>,
) -> std::ops::Range<usize> {
    if all_are_options {
        // `range_emptying_hash`: the hash goes away with the comma in front of it, when there is an
        // argument to hang that comma off.
        return match preceding {
            Some(argument) => argument.end_byte()..pair.end_byte(),
            None => pair.byte_range(),
        };
    }
    // `range_within_hash`: the pair leaves with the comma on whichever side it has.
    let position = pairs.iter().position(|entry| entry.id() == pair.id());
    match position {
        Some(index) if index > 0 => pairs[index - 1].end_byte()..pair.end_byte(),
        Some(index) if index + 1 < pairs.len() => pair.start_byte()..pairs[index + 1].start_byte(),
        _ => pair.byte_range(),
    }
}
