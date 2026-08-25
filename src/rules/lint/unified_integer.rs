use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The last version where `Fixnum` and `Bignum` were still distinct classes.
const UNIFIED_SINCE: crate::ruby_version::RubyVersion = crate::ruby_version::RubyVersion::new(2, 3);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["constant", "scope_resolution"]) {
        let Some(name) = short_name(node) else {
            continue;
        };
        let text = context.source.node_text(name);
        if !matches!(text, "Fixnum" | "Bignum") {
            continue;
        }
        let offense = context.offense(
            format!("Use `Integer` instead of `{text}`."),
            node.byte_range(),
        );
        // **`next if target_ruby_version <= 2.3`.** `Integer` did not yet absorb `Fixnum` there,
        // so the offense stands but the rewrite is withheld -- and the report says `correctable`
        // is false.
        offenses.push(match context.target_ruby_version() <= UNIFIED_SINCE {
            true => offense,
            false => offense.corrected_by(Edit {
                start: name.start_byte(),
                end: name.end_byte(),
                replacement: "Integer".to_owned(),
                safe: true,
            }),
        });
    }
}

/// `node.loc.name` for the constants the pattern `(const {nil? (cbase)} _)` reaches: a bare name,
/// or one written after `::` with nothing in front of it. A name qualified by a scope is a
/// different constant, and the name token inside it is no constant of its own.
fn short_name<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "constant" => {
            let qualified = node.parent().is_some_and(|parent| {
                parent.kind_str() == "scope_resolution"
                    && parent
                        .field("name")
                        .is_some_and(|name| name.id() == node.id())
            });
            (!qualified).then_some(node)
        }
        "scope_resolution" if node.field("scope").is_none() => {
            node.field("name")
        }
        _ => None,
    }
}
