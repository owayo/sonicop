use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The values of `PreferredMethods` in the bundled default configuration.
const DEFAULT_PREFERENCES: [&str; 7] = [
    "map", "map!", "flat_map", "reduce", "find", "select", "include?",
];

/// A call is checked when it carries a block, or when it takes one implicitly -- a `&block` pass, or
/// a symbol handed to one of `MethodsAcceptingSymbol`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(preferences) =
        super::method_preference::preferred_methods(context, &DEFAULT_PREFERENCES)
    else {
        return;
    };
    let accepting_symbol = context
        .setting::<Vec<String>>("MethodsAcceptingSymbol")
        .unwrap_or_default();
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let current = context.source.node_text(selector);
        let Some(prefer) = preferences.get(current) else {
            continue;
        };
        if node.field("block").is_none()
            && !takes_block_implicitly(node, context, &accepting_symbol)
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Prefer `{prefer}` over `{current}`."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: prefer.clone(),
                    safe: true,
                }),
        );
    }
}

/// `implicit_block?`.
fn takes_block_implicitly(
    node: tree_sitter::Node<'_>,
    context: &RuleContext<'_>,
    accepting_symbol: &[String],
) -> bool {
    let arguments = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    let Some(last) = arguments.last() else {
        return false;
    };
    if last.kind_str() == "block_argument" {
        return true;
    }
    if !matches!(last.kind_str(), "simple_symbol" | "delimited_symbol") {
        return false;
    }
    let Some(selector) = node.field("method") else {
        return false;
    };
    accepting_symbol
        .iter()
        .any(|name| name == context.source.node_text(selector))
}
