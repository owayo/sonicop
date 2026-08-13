use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, is_string, string_text};

use super::ranges::{whole_lines, with_space_on_right};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Remove unnecessary `require` statement.";

/// The modifier keywords, whose node answers `modifier_form?`. A `require` written under one has
/// to keep the conditional it was the body of.
const MODIFIERS: &[&str] = &[
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if !is_plain_send(call, context) || call.field("receiver").is_some() {
            continue;
        }
        let Some(selector) = call.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "require" {
            continue;
        }
        let call_arguments = arguments(call);
        let [feature] = call_arguments.as_slice() else {
            continue;
        };
        let feature = feature.first();
        if feature.kind_str() == "identifier" || !is_string(feature, context) {
            continue;
        }
        if !is_redundant(string_text(feature, context), context.target_ruby_version()) {
            continue;
        }
        offenses.push(match modifier_parent(call) {
            Some(parent) => context
                .offense(MSG, call.byte_range())
                .corrections_anchored_at(parent.byte_range())
                .corrected_by_all([
                    Edit {
                        start: parent.end_byte(),
                        end: parent.end_byte(),
                        replacement: "\nend".to_owned(),
                        safe: true,
                    },
                    remove(with_space_on_right(call.byte_range(), context)),
                ]),
            None => context
                .offense(MSG, call.byte_range())
                .corrected_by(remove(whole_lines(call.byte_range(), context))),
        });
    }
}

fn remove(range: std::ops::Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

/// The conditional the `require` is the body of, when it was written as a modifier.
fn modifier_parent<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let parent = call.parent()?;
    MODIFIERS.contains(&parent.kind_str()).then_some(parent)
}

/// `redundant_feature?`: a file the interpreter has already loaded by the version being targeted.
fn is_redundant(feature: &str, version: RubyVersion) -> bool {
    match feature {
        "enumerator" => true,
        "thread" => version >= RubyVersion::new(2, 1),
        "rational" | "complex" => version >= RubyVersion::new(2, 2),
        "ruby2_keywords" => version >= RubyVersion::new(2, 7),
        "fiber" => version >= RubyVersion::new(3, 1),
        "set" => version >= RubyVersion::new(3, 2),
        "pathname" => version >= RubyVersion::new(4, 0),
        _ => false,
    }
}
