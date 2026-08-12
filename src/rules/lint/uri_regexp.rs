use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range, top_level_constant};

/// `URI::RFC2396_PARSER` replaced `URI::DEFAULT_PARSER` in Ruby 3.4, so the replacement upstream
/// names depends on the version the run targets.
const RFC2396_SINCE: RubyVersion = RubyVersion::new(3, 4);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(method) != "regexp" || !is_plain_send(node, context) {
            continue;
        }
        let Some(receiver) = node.child_by_field_name("receiver") else {
            continue;
        };
        if !top_level_constant(receiver, "URI", context) {
            continue;
        }
        let range = send_range(node, context);
        let preferred = preferred(node, receiver, context);
        let message = format!(
            "`{}` is obsolete and should not be used. Instead, use `{preferred}`.",
            context.source.slice(range.clone()),
        );
        offenses.push(
            context
                .offense(message, method.byte_range())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: preferred,
                    safe: true,
                }),
        );
    }
}

/// `"#{node.receiver.source}::#{parser}.make_regexp#{argument}"`: the call rewritten onto the
/// parser constant, keeping the argument list only when one was written.
fn preferred(node: Node<'_>, receiver: Node<'_>, context: &RuleContext<'_>) -> String {
    let parser = if context.target_ruby_version() >= RFC2396_SINCE {
        "RFC2396_PARSER"
    } else {
        "DEFAULT_PARSER"
    };
    let argument = arguments(node).first().map_or_else(String::new, |first| {
        format!("({})", context.source.slice(first.range()))
    });
    format!(
        "{}::{parser}.make_regexp{argument}",
        context.source.node_text(receiver),
    )
}
