use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{is_plain_send, send_range};

/// `RESTRICT_ON_SEND`, and the two replacement lists keyed by which half of it the method is in.
const ESCAPING: [&str; 2] = ["escape", "encode"];
const UNESCAPING: [&str; 2] = ["unescape", "decode"];

const ESCAPE_REPLACEMENTS: &str = "`CGI.escape`, `URI.encode_www_form` or \
                                   `URI.encode_www_form_component`";
const UNESCAPE_REPLACEMENTS: &str = "`CGI.unescape`, `URI.decode_www_form` or \
                                     `URI.decode_www_form_component`";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        let name = context.source.node_text(method);
        let replacements = if ESCAPING.contains(&name) {
            ESCAPE_REPLACEMENTS
        } else if UNESCAPING.contains(&name) {
            UNESCAPE_REPLACEMENTS
        } else {
            continue;
        };
        if !is_plain_send(node, context) {
            continue;
        }
        // `(const ${nil? cbase} :URI)`: the capture is the `cbase` node, so it is truthy only for
        // `::URI`, which is the difference the message spells out.
        let Some(receiver) = node.child_by_field_name("receiver") else {
            continue;
        };
        let double_colon = match receiver.kind() {
            "constant" if context.source.node_text(receiver) == "URI" => "",
            "scope_resolution"
                if receiver.child_by_field_name("scope").is_none()
                    && receiver
                        .child_by_field_name("name")
                        .is_some_and(|inner| context.source.node_text(inner) == "URI") =>
            {
                "::"
            }
            _ => continue,
        };
        let message = format!(
            "`{double_colon}URI.{name}` method is obsolete and should not be used. Instead, use \
             {replacements} depending on your specific use case."
        );
        offenses.push(context.offense(message, send_range(node, context)));
    }
}
