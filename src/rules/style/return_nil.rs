use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const RETURN_MSG: &str = "Use `return` instead of `return nil`.";
const RETURN_NIL_MSG: &str = "Use `return nil` instead of `return`.";

/// `on_return`: whether a `return` should carry an explicit `nil`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let want_nil = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "return_nil");
    for node in context.nodes_of("return") {
        let written = super::nodes::children_in(node, context);
        let returns_nil = matches!(
            written.as_slice(),
            [list] if list.kind_str() == "argument_list"
                && matches!(super::nodes::children_in(*list, context).as_slice(),
                            [only] if only.kind_str() == "nil")
        );
        // `correct_style?`: only one of the two spellings is ever wrong, and a `return 1` is
        // neither of them.
        let wrong = if want_nil {
            written.is_empty()
        } else {
            returns_nil
        };
        if !wrong {
            continue;
        }
        if inside_a_yielding_block(node, context) {
            continue;
        }
        let (message, replacement) = if want_nil {
            (RETURN_NIL_MSG, "return nil")
        } else {
            (RETURN_MSG, "return")
        };
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: replacement.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// The ancestor walk upstream runs before reporting: a `return` inside a block that takes
/// parameters and hangs off a call with a receiver is left alone, because the value it returns goes
/// to that call.
fn inside_a_yielding_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    while let Some(ancestor) = current.parent() {
        current = ancestor;
        match ancestor.kind_str() {
            // `scoped_node?`: a method definition ends the walk.
            "method" | "singleton_method" => return false,
            "block" | "do_block" => {}
            _ => continue,
        }
        let Some(call) = ancestor.parent() else {
            return false;
        };
        // A `-> { }` is a block whose `lambda?` is true, which also ends the walk.
        if call.kind_str() == "lambda" {
            return false;
        }
        if call.kind_str() != "call" {
            continue;
        }
        let selector = call.field("method").map(|node| context.source.node_text(node));
        if selector == Some("lambda") && call.field("receiver").is_none() {
            return false;
        }
        // `define_method?`: `(send _ {:define_method :define_singleton_method} _)`.
        if matches!(selector, Some("define_method" | "define_singleton_method"))
            && call
                .field("arguments")
                .map(super::nodes::children)
                .is_some_and(|arguments| arguments.len() == 1)
        {
            return false;
        }
        // `next if args_node.children.empty?`: a block without parameters keeps the walk going.
        if ancestor.field("parameters").is_none() {
            continue;
        }
        // `chained_send?`: `(send !nil? ...)`.
        if call.field("receiver").is_some() {
            return true;
        }
    }
    false
}
