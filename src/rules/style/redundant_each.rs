use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Remove redundant `each`.";
const MSG_WITH_INDEX: &str = "Use `with_index` to remove redundant `each`.";
const MSG_WITH_OBJECT: &str = "Use `with_object` to remove redundant `each`.";

const RESTRICTED: [&str; 3] = ["each", "each_with_index", "each_with_object"];

/// An `each` that another enumeration method makes pointless, in either direction: `each` in front
/// of one, or one in front of `each_with_index` / `each_with_object`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !RESTRICTED.contains(&name) {
            continue;
        }
        let Some(redundant) = redundant_each(node, name, context) else {
            continue;
        };
        let Some(range) = offense_range(node, name, selector) else {
            continue;
        };
        let message = match name {
            "each" => MSG,
            "each_with_index" => MSG_WITH_INDEX,
            _ => MSG_WITH_OBJECT,
        };
        let offense = context.offense(message, range.clone());
        offenses.push(match name {
            "each" => {
                // The `each` goes away; the method it was in front of takes over its job.
                let mut edits = vec![Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }];
                if let Some(inner) = redundant.field("method") {
                    let replacement = match context.source.node_text(inner) {
                        "each_with_index" => Some("each.with_index"),
                        "each_with_object" => Some("each.with_object"),
                        _ => None,
                    };
                    if let Some(replacement) = replacement {
                        edits.push(Edit {
                            start: inner.start_byte(),
                            end: inner.end_byte(),
                            replacement: replacement.to_owned(),
                            safe: true,
                        });
                    }
                }
                offense.corrected_by_all(edits)
            }
            "each_with_index" => offense.corrected_by(Edit {
                start: selector.start_byte(),
                end: selector.end_byte(),
                replacement: "with_index".to_owned(),
                safe: true,
            }),
            _ => offense.corrected_by(Edit {
                start: selector.start_byte(),
                end: selector.end_byte(),
                replacement: "with_object".to_owned(),
                safe: true,
            }),
        });
    }
}

/// `redundant_each_method`: the call whose presence makes this one redundant.
fn redundant_each<'tree>(
    node: Node<'tree>,
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if last_argument_is_block_pass(node) {
        return None;
    }
    // An `each` written in front of another enumeration method, which is the one to keep.
    if name == "each" && node.field("block").is_none() {
        if let Some(parent) = node.parent().filter(|parent| {
            parent.kind_str() == "call"
                && parent
                    .field("receiver")
                    .is_some_and(|inner| inner.id() == node.id())
        }) {
            let following = parent
                .field("method")
                .map(|method| context.source.node_text(method));
            if following
                .is_some_and(|method| RESTRICTED.contains(&method) || method == "reverse_each")
            {
                return Some(parent);
            }
        }
    }
    // Otherwise the method in front of this one is what makes it redundant.
    let previous = node.field("receiver")?;
    if previous.kind_str() != "call"
        || previous.field("block").is_some()
        || last_argument_is_block_pass(previous)
    {
        return None;
    }
    let earlier = context.source.node_text(previous.field("method")?);
    // `detected` is only computed for the `each_with_*` selectors, so a plain `each` needs the
    // previous call to be `reverse_each`.
    let detected = name != "each" && earlier.starts_with("each_");
    (detected || earlier == "reverse_each").then_some(previous)
}

/// `range(node)`.
fn offense_range(
    node: Node<'_>,
    name: &str,
    selector: Node<'_>,
) -> Option<std::ops::Range<usize>> {
    if name != "each" {
        return Some(selector.byte_range());
    }
    match node.parent().filter(|parent| parent.kind_str() == "call") {
        // `node.selector.join(node.parent.loc.dot)`: the `each` and the dot after it.
        Some(parent) => {
            let dot = parent.field("operator")?;
            Some(selector.start_byte()..dot.end_byte())
        }
        // `node.loc.dot.join(node.selector)`: the dot in front of the `each` and the `each`.
        None => {
            let dot = node.field("operator")?;
            Some(dot.start_byte()..selector.end_byte())
        }
    }
    .filter(|range| range.start <= range.end)
}

/// `node.last_argument&.block_pass_type?`.
fn last_argument_is_block_pass(node: Node<'_>) -> bool {
    node.field("arguments")
        .map(super::nodes::children)
        .and_then(|arguments| arguments.last().copied())
        .is_some_and(|last| last.kind_str() == "block_argument")
}
