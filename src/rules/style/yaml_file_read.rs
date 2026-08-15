use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `safe_load_file` did not exist before Ruby 3.0, so the suggestion is withheld at 2.7 and below.
const SAFE_LOAD_MINIMUM: RubyVersion = RubyVersion::new(2, 7);

/// `(send (const {cbase nil?} :YAML) _ (send (const {cbase nil?} :File) :read $_) $...)`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method"))
        else {
            continue;
        };
        let method = context.source.node_text(selector);
        if !matches!(method, "load" | "safe_load" | "parse") {
            continue;
        }
        if method == "safe_load" && context.target_ruby_version() <= SAFE_LOAD_MINIMUM {
            continue;
        }
        // `(send ...)`: a `&.` call is a `csend` upstream and never matches the pattern.
        if !send_node::is_plain_send(node, context) {
            continue;
        }
        if !send_node::top_level_constant(receiver, "YAML", context) {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let [first, rest @ ..] = arguments.as_slice() else {
            continue;
        };
        let Some(path) = file_read_path(*first, context) else {
            continue;
        };
        let rest = if rest.is_empty() {
            String::new()
        } else {
            let written: Vec<&str> = rest
                .iter()
                .map(|argument| context.source.node_text(*argument))
                .collect();
            format!(", {}", written.join(", "))
        };
        let prefer = format!("{method}_file({}{rest})", context.source.node_text(path));
        // `node.loc.selector.join(node.source_range.end)`: the receiver stays, so `YAML.` is kept
        // and only the call after it is rewritten.
        let range = selector.start_byte()..send_node::send_range(node, context).end;
        offenses.push(
            context
                .offense(format!("Use `{prefer}` instead."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: prefer,
                    safe: true,
                }),
        );
    }
}

/// The single argument of a `(send (const {cbase nil?} :File) :read $_)`.
fn file_read_path<'tree>(
    node: tree_sitter::Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<tree_sitter::Node<'tree>> {
    if node.kind_str() != "call" || node.field("block").is_some() {
        return None;
    }
    if !send_node::is_plain_send(node, context) {
        return None;
    }
    if !send_node::top_level_constant(node.field("receiver")?, "File", context) {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "read" {
        return None;
    }
    match super::nodes::children(node.field("arguments")?).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}
