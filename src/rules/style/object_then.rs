use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 2.6`: `Object#then` landed in 2.6.
const MINIMUM: RubyVersion = RubyVersion::new(2, 6);

const NAMES: [&str; 2] = ["then", "yield_self"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "then".to_owned());
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !NAMES.contains(&name) {
            continue;
        }
        // `on_block` takes any block written on the call; `on_send` only takes a call whose one
        // argument is a `&block` pass. A bare `x.then` is neither and is left alone.
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let block_pass =
            matches!(arguments.as_slice(), [only] if only.kind_str() == "block_argument");
        if node.field("block").is_none() && !block_pass {
            continue;
        }
        if name == style {
            continue;
        }
        // `style == :then && node.receiver.nil? ? 'self.then' : style`: without a receiver the bare
        // `then` would read as the keyword, so the explicit receiver is written in.
        let prefer = if style == "then" && node.field("receiver").is_none() {
            "self.then".to_owned()
        } else {
            style.clone()
        };
        let range = selector.byte_range();
        offenses.push(
            context
                .offense(format!("Prefer `{style}` over `{name}`."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: prefer,
                    safe: true,
                }),
        );
    }
}
