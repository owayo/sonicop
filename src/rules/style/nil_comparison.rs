use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const PREDICATE_MSG: &str = "Prefer the use of the `nil?` predicate.";
const EXPLICIT_MSG: &str = "Prefer the use of the `==` comparison.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let comparison = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "comparison");

    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((receiver, selector, argument)) = parts(node) else {
            continue;
        };
        let name = context.source.node_text(selector);
        let edit = match comparison {
            // `(send _ :nil?)`: the predicate becomes an equality test.
            true => {
                if name != "nil?" || argument.is_some() {
                    continue;
                }
                // `node.loc.dot.join(node.loc.selector.end)`.
                let Some(dot) = node.field("operator") else {
                    continue;
                };
                Edit {
                    start: dot.start_byte(),
                    end: selector.end_byte(),
                    replacement: " == nil".to_owned(),
                    safe: true,
                }
            }
            // `(send _ {:== :===} nil)`.
            false => {
                if !matches!(name, "==" | "===")
                    || argument.is_none_or(|argument| argument.kind_str() != "nil")
                {
                    continue;
                }
                Edit {
                    start: receiver.end_byte(),
                    end: node.end_byte(),
                    replacement: ".nil?".to_owned(),
                    safe: true,
                }
            }
        };
        let message = match comparison {
            true => EXPLICIT_MSG,
            false => PREDICATE_MSG,
        };
        let mut edits = vec![edit];
        // `corrector.wrap(node, '(', ')')`: the rewrite binds differently under a `!`.
        if node.parent_of(context).is_some_and(|parent| is_negation(context, parent, node)) {
            edits.push(Edit {
                start: node.end_byte(),
                end: node.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            });
            edits.push(Edit {
                start: node.start_byte(),
                end: node.start_byte(),
                replacement: "(".to_owned(),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(message, selector.byte_range())
                .corrected_by_all(edits)
                .corrections_anchored_at(node.byte_range()),
        );
    }
}

/// The receiver, selector and single argument of a call written either way round, with the shapes
/// that have no receiver or more than one argument ruled out.
fn parts<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, Node<'tree>, Option<Node<'tree>>)> {
    match node.kind_str() {
        "binary" => Some((
            node.field("left")?,
            node.field("operator")?,
            Some(node.field("right")?),
        )),
        _ => {
            if node.field("block").is_some() {
                return None;
            }
            let receiver = node.field("receiver")?;
            let selector = node.field("method")?;
            let argument = match node.field("arguments") {
                Some(arguments) => match super::nodes::children(arguments).as_slice() {
                    [only] => Some(*only),
                    _ => return None,
                },
                None => None,
            };
            Some((receiver, selector, argument))
        }
    }
}

/// `parent.method?(:!)`, which is how the parser spells both `!x` and `not x`.
fn is_negation(context: &RuleContext<'_>, parent: Node<'_>, node: Node<'_>) -> bool {
    parent.kind_str() == "unary"
        && parent
            .field("operand")
            .is_some_and(|operand| operand.id() == node.id())
        && parent
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}
