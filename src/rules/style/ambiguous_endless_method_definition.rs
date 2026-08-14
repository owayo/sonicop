//! `Style/AmbiguousEndlessMethodDefinition`: what a modifier after an endless method attaches to.

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 3.0`: endless methods arrived in 3.0.
const MINIMUM: RubyVersion = RubyVersion::new(3, 0);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        if !super::endless::is_endless(node) {
            continue;
        }
        let Some(operation) = context.parent(node) else {
            continue;
        };
        // `^${(if _ <def _>) ({and or} def _) ({while until} _ def)}`: the definition has to be a
        // branch of the conditional, the left of the operator, or the body of the loop -- never the
        // condition. Only the modifier forms are ambiguous, and an `and`/`or` always is.
        let keyword = match operation.kind_str() {
            "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier" => {
                if operation
                    .field("body")
                    .is_none_or(|body| body.id() != node.id())
                {
                    continue;
                }
                super::conditional::token(operation, &["if", "unless", "while", "until"])
                    .map(|keyword| context.source.node_text(keyword))
            }
            "binary" => {
                let Some(operator) = operation.field("operator") else {
                    continue;
                };
                if !matches!(
                    context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                ) {
                    continue;
                }
                if operation
                    .field("left")
                    .is_none_or(|left| left.id() != node.id())
                {
                    continue;
                }
                Some(context.source.node_text(operator))
            }
            _ => continue,
        };
        let Some(keyword) = keyword else {
            continue;
        };
        let offense = context.offense(
            format!("Avoid using `{keyword}` statements with endless methods."),
            operation.byte_range(),
        );
        offenses.push(match super::endless::correct_to_multiline(context, node) {
            Some(replacement) => offense.corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
            None => offense,
        });
    }
}
