use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use crate::ruby_version::RubyVersion;
use crate::rules::node_ext::NodeExt;

const NUMBERED_VERSION: RubyVersion = RubyVersion::new(2, 7);
const IT_VERSION: RubyVersion = RubyVersion::new(3, 4);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(block) = node.field("block") else {
            continue;
        };
        let Some(method) = node.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "sort_by" {
            continue;
        }
        // The body has to be the one variable the block declares and nothing else.
        let statements = body_statements(block);
        let [statement] = statements.as_slice() else {
            continue;
        };
        if statement.kind_str() != "identifier" {
            continue;
        }
        let read = context.source.node_text(*statement);
        let message = match block.field("parameters") {
            Some(parameters) => match super::nodes::children_in(parameters, context).as_slice() {
                [only]
                    if only.kind_str() == "identifier" && context.source.node_text(*only) == read =>
                {
                    format!("Use `sort` instead of `sort_by {{ |{read}| {read} }}`.")
                }
                _ => continue,
            },
            // `numblock` and `itblock` read their one parameter without declaring it.
            None if read == "_1" && context.target_ruby_version() >= NUMBERED_VERSION => {
                "Use `sort` instead of `sort_by { _1 }`.".to_owned()
            }
            None if read == "it" && context.target_ruby_version() >= IT_VERSION => {
                "Use `sort` instead of `sort_by { it }`.".to_owned()
            }
            None => continue,
        };
        // `sort_by_range`: from the selector to the block's closing delimiter.
        let range = method.start_byte()..block.end_byte();
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: "sort".to_owned(),
            safe: true,
        }));
    }
}

/// The statements a block body holds, which the grammar wraps in a node of its own.
fn body_statements<'tree>(block: Node<'tree>) -> Vec<Node<'tree>> {
    match block.field("body") {
        Some(body) => match body.kind_str() {
            "block_body" | "body_statement" => super::nodes::children(body),
            _ => vec![body],
        },
        None => Vec::new(),
    }
}
