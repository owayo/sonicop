use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{is_plain_send, top_level_constant};

/// `maximum_target_ruby_version 3.0`. Psych 4, which ships with Ruby 3.1, already loads safely, so
/// upstream retires the cop rather than asking for a method that is no longer the safer one.
const MAXIMUM_TARGET_RUBY: RubyVersion = RubyVersion::new(3, 0);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() > MAXIMUM_TARGET_RUBY {
        return;
    }
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(method) != "load" || !is_plain_send(node, context) {
            continue;
        }
        if !node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "YAML", context))
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    "Prefer using `YAML.safe_load` over `YAML.load`.",
                    method.byte_range(),
                )
                .corrected_by(Edit {
                    start: method.start_byte(),
                    end: method.end_byte(),
                    replacement: "safe_load".to_owned(),
                    safe: true,
                }),
        );
    }
}
