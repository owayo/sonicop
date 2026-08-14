use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use parentheses for `super` with arguments.";

/// `on_super`: a `super` that forwards written-out arguments. The bare `super` is a `zsuper`
/// upstream and never reaches this cop, which is why only the calls that carry an argument list
/// are walked here.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(keyword) = node.field("method") else {
            continue;
        };
        if keyword.kind_str() != "super" {
            continue;
        }
        let Some(arguments) = node.field("arguments") else {
            continue;
        };
        // `node.parenthesized?`: the argument list starts at the paren when there is one, so its
        // first byte is what decides.
        if context.source.node_text(arguments).starts_with('(') {
            continue;
        }
        let written = super::nodes::children(arguments);
        let (Some(first), Some(last)) = (written.first(), written.last()) else {
            continue;
        };
        // Upstream's `super` node ends where its arguments do -- a block written after it belongs
        // to the `block` node wrapped around it -- so the report stops at the last argument.
        let range = node.start_byte()..last.end_byte();
        offenses.push(context.offense(MSG, range).corrected_by_all([
            // `corrector.replace(keyword.end.join(first_argument.begin), '(')`: the space between
            // the keyword and the arguments becomes the opening paren.
            Edit {
                start: keyword.end_byte(),
                end: first.start_byte(),
                replacement: "(".to_owned(),
                safe: true,
            },
            // `corrector.insert_after(last_argument, ')')`.
            Edit {
                start: last.end_byte(),
                end: last.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            },
        ]));
    }
}
