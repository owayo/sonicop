//! `Style/DataInheritance`: what `Data.define` returns is a class already.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::final_pos;

const MSG: &str = "Don't extend an instance initialized by `Data.define`. Use a block to \
                   customize the class.";

/// `minimum_target_ruby_version 3.2`: `Data` arrived in 3.2.
const MINIMUM: RubyVersion = RubyVersion::new(3, 2);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of("class") {
        let Some(superclass) = node.field("superclass") else {
            continue;
        };
        let parts = super::nodes::children(superclass);
        let [parent] = parts.as_slice() else {
            continue;
        };
        if !is_data_define(*parent, context) {
            continue;
        }
        let (Some(keyword), Some(operator)) = (
            super::conditional::token(node, &["class"]),
            super::conditional::token(superclass, &["<"]),
        ) else {
            continue;
        };
        let text = context.source.text();
        let mut edits = vec![
            // `range_with_surrounding_space(node.loc.keyword, newlines: false)`.
            Edit {
                start: final_pos(text, keyword.start_byte(), false, false, false, false),
                end: final_pos(text, keyword.end_byte(), true, false, false, false),
                replacement: String::new(),
                safe: true,
            },
            Edit {
                start: operator.start_byte(),
                end: operator.end_byte(),
                replacement: "=".to_owned(),
                safe: true,
            },
        ];
        // `correct_parent`: a `Data.define` that already carries a block keeps it, and the class's
        // own `end` closes it; otherwise the `end` becomes a block's or goes away with the class.
        match (parent.field("block"), node.field("body")) {
            (Some(block), _) => {
                if let Some(open) = super::conditional::token(block, &["{"]) {
                    edits.push(Edit {
                        start: open.start_byte(),
                        end: open.end_byte(),
                        replacement: "do".to_owned(),
                        safe: true,
                    });
                }
                if let Some(close) = super::conditional::token(block, &["}", "end"]) {
                    edits.push(Edit {
                        start: final_pos(text, close.start_byte(), false, false, false, false),
                        end: final_pos(text, close.end_byte(), true, false, false, false),
                        replacement: String::new(),
                        safe: true,
                    });
                }
            }
            (None, Some(_)) => edits.push(Edit {
                start: parent.end_byte(),
                end: parent.end_byte(),
                replacement: " do".to_owned(),
                safe: true,
            }),
            (None, None) => edits.push(empty_body_removal(node, *parent, context)),
        }
        offenses.push(
            context
                .offense(MSG, parent.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `range_for_empty_class_body`: what is left of a class with nothing in it.
fn empty_body_removal(class: Node<'_>, parent: Node<'_>, context: &RuleContext<'_>) -> Edit {
    let range = if class.start_position().row == class.end_position().row {
        parent.end_byte()..class.end_byte()
    } else {
        // `range_by_whole_lines(class_node.loc.end, include_final_newline: true)`.
        let (line, _) = context
            .source
            .line_column(class.end_byte().saturating_sub(1));
        context.source.line_range(line)
    };
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// `{(send (const {nil? cbase} :Data) :define ...) (block (send ...) ...)}`.
fn is_data_define(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && node
            .field("method")
            .is_some_and(|name| context.source.node_text(name) == "define")
        && node
            .field("receiver")
            .is_some_and(|receiver| super::nodes::is_top_level_constant(receiver, "Data", context))
}
