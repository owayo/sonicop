use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The filtering methods a predicate can be folded into.
const FILTERS: [&str; 3] = ["select", "filter", "find_all"];

/// `REPLACEMENT_METHODS`, with the two entries `RAILS_METHODS` guards.
const REPLACEMENTS: [(&str, &str, bool); 6] = [
    ("any?", "any?", false),
    ("empty?", "none?", false),
    ("none?", "none?", false),
    ("one?", "one?", false),
    ("many?", "many?", true),
    ("present?", "any?", true),
];

/// `select { ... }.any?` and the like, which the predicate can do on its own.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let active_support = context
        .setting_of::<bool>("AllCops", "ActiveSupportExtensionsEnabled")
        .unwrap_or(false);
    for node in context.nodes_of("call") {
        let Some(predicate) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(predicate);
        let Some((_, replacement, rails)) =
            REPLACEMENTS.iter().find(|(current, _, _)| *current == name)
        else {
            continue;
        };
        if *rails && !active_support {
            continue;
        }
        // `node.arguments?` / `node.block_literal?`: the predicate has to stand alone.
        if node.field("arguments").is_some() || node.field("block").is_some() {
            continue;
        }
        let (Some(receiver), Some(operator)) = (node.field("receiver"), node.field("operator"))
        else {
            continue;
        };
        let Some(filter) = filtering_selector(receiver, context) else {
            continue;
        };
        // `select_node.loc.selector.join(predicate_node.loc.selector)`.
        let reported = filter.start_byte()..predicate.end_byte();
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{replacement}` instead of `{}.{name}`.",
                        context.source.node_text(filter)
                    ),
                    reported,
                )
                .corrected_by_all([
                    // `predicate_node.receiver.source_range.end.join(predicate_node.loc.selector)`:
                    // the dot and the predicate go away together.
                    Edit {
                        start: operator.start_byte().min(receiver.end_byte()),
                        end: predicate.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: filter.start_byte(),
                        end: filter.end_byte(),
                        replacement: (*replacement).to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// The `select` / `filter` / `find_all` selector of the receiver.
///
/// Upstream accepts two shapes: the call carrying a block and no arguments, or the call whose only
/// argument is a `&block` pass. Both node patterns spell out the argument list exactly, so a
/// `select(x) { ... }` is not one of them.
fn filtering_selector<'tree>(
    receiver: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if receiver.kind_str() != "call" {
        return None;
    }
    let selector = receiver.field("method")?;
    if !FILTERS.contains(&context.source.node_text(selector)) {
        return None;
    }
    let arguments = receiver
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    let matched = if receiver.field("block").is_some() {
        arguments.is_empty()
    } else {
        matches!(arguments.as_slice(), [only] if only.kind_str() == "block_argument")
    };
    matched.then_some(selector)
}
