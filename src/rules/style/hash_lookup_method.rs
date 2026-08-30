use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const BRACKET_MSG: &str = "Use `Hash#[]` instead of `Hash#fetch`.";
const FETCH_MSG: &str = "Use `Hash#fetch` instead of `Hash#[]`.";

/// Either `Hash#[]` or `Hash#fetch`, whichever the configuration asks for.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let want_fetch = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "fetch");
    let allowed = context
        .setting::<Vec<String>>("AllowedReceivers")
        .unwrap_or_default();
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let receiver = node.field("receiver").or_else(|| node.field("object"));
        if let Some(receiver) = receiver
            && allowed
                .iter()
                .any(|entry| *entry == receiver_name(receiver, context))
        {
            continue;
        }
        let offense = if want_fetch {
            fetch_offense(node, context)
        } else {
            bracket_offense(node, receiver, context)
        };
        if let Some(offense) = offense {
            offenses.push(offense);
        }
    }
}

/// `offense_for_brackets?`: a one-argument `fetch` written with `.`, and no block.
fn bracket_offense(
    node: Node<'_>,
    receiver: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    if node.kind_str() != "call" {
        return None;
    }
    let receiver = receiver?;
    let selector = node.field("method")?;
    if context.source.node_text(selector) != "fetch" {
        return None;
    }
    if node.field("block").is_some() || !send_node::is_plain_send(node, context) {
        return None;
    }
    let arguments = super::nodes::children_in(node.field("arguments")?, context);
    let [key] = arguments.as_slice() else {
        return None;
    };
    // `node.loc.dot.join(node.source_range.end)`: the dot through the end of the call.
    let start = node
        .field("operator")
        .map_or_else(|| receiver.end_byte(), |operator| operator.start_byte());
    Some(
        context
            .offense(BRACKET_MSG, selector.byte_range())
            .corrected_by(Edit {
                start,
                end: send_node::send_range(node, context).end,
                replacement: format!("[{}]", context.source.node_text(*key)),
                safe: true,
            }),
    )
}

/// `offense_for_fetch?`: a one-argument `[]`, in either of the two spellings the grammar has for it.
fn fetch_offense(node: Node<'_>, context: &RuleContext<'_>) -> Option<Offense> {
    // `node.method?(:[])`: written on the left of an assignment the same brackets are `[]=`, which
    // `fetch` has no counterpart for. The grammar spells both as an `element_reference`, so the
    // parent has to be asked which one this is.
    if is_assignment_target(node, context) {
        return None;
    }
    let (key, selector_start, safe_navigation) = match node.kind_str() {
        "element_reference" => {
            let object = node.field("object")?;
            let indices = super::nodes::children_in(node, context);
            let [_, key] = indices.as_slice() else {
                return None;
            };
            // `node.loc.selector` for a `send :[]` is the whole `[key]`.
            let bracket = context.source.text()[object.end_byte()..node.end_byte()].find('[')?;
            (*key, object.end_byte() + bracket, false)
        }
        _ => {
            let selector = node.field("method")?;
            if context.source.node_text(selector) != "[]" {
                return None;
            }
            let arguments = super::nodes::children_in(node.field("arguments")?, context);
            let [key] = arguments.as_slice() else {
                return None;
            };
            let safe = !send_node::is_plain_send(node, context);
            let start = if safe {
                node.field("operator")?.start_byte()
            } else {
                selector.start_byte()
            };
            (*key, start, safe)
        }
    };
    let written = context.source.node_text(key);
    let replacement = if safe_navigation {
        format!("&.fetch({written})")
    } else {
        format!(".fetch({written})")
    };
    Some(
        context
            .offense(FETCH_MSG, node.byte_range())
            .corrected_by(Edit {
                start: selector_start,
                end: send_node::send_range(node, context).end,
                replacement,
                safe: true,
            }),
    )
}

/// `AllowedReceivers#receiver_name`.
fn receiver_name(receiver: Node<'_>, context: &RuleContext<'_>) -> String {
    if let Some(inner) = receiver.field("receiver")
        && inner.kind_str() != "constant"
        && inner.kind_str() != "scope_resolution"
    {
        return receiver_name(inner, context);
    }
    if receiver.kind_str() == "call" {
        let selector = receiver
            .field("method")
            .map(|node| context.source.node_text(node))
            .unwrap_or_default();
        return match receiver.field("receiver") {
            Some(inner) => format!("{}.{selector}", receiver_name(inner, context)),
            None => selector.to_owned(),
        };
    }
    context.source.node_text(receiver).to_owned()
}

/// Whether the node stands on the left of an assignment, where its brackets mean `[]=`.
///
/// Only a plain assignment counts. `x[k] += 1` is `(op-asgn (send x :[] k) :+ 1)` upstream -- the
/// read is a `send :[]` of its own, and the cop reports it -- while `x[k] = 1` is a single
/// `send :[]=` with no read in it at all. A multiple assignment writes through `[]=` as well, so
/// its target list is excluded too.
fn is_assignment_target(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    match parent.kind_str() {
        // The grammar also spells `value[index] =~ /pattern/` as an `assignment`: `=` is the
        // assignment token and `~ /pattern/` its unary right side. With no whitespace between
        // those two characters Ruby reads the pair as the match operator, so the brackets remain
        // a `send :[]` upstream and must not be mistaken for an `[]=` target.
        "assignment" => {
            if parent.field("left") != Some(node) {
                return false;
            }
            let match_operator = parent.field("right").is_some_and(|right| {
                right.kind_str() == "unary"
                    && right
                        .field("operator")
                        .is_some_and(|operator| context.source.node_text(operator) == "~")
                    && right.start_byte() > 0
                    && context.source.text().as_bytes()[right.start_byte() - 1] == b'='
            });
            !match_operator
        }
        "left_assignment_list" | "rest_assignment" => true,
        _ => false,
    }
}
