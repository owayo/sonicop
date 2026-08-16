use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // The indirect inheritance the cop can also report needs `AllCops/UseProjectIndex` and the
    // `rubydex` gem to index the project; without an index the check never runs.
    let preferred = match context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "standard_error".to_owned())
        .as_str()
    {
        "runtime_error" => "RuntimeError",
        _ => "StandardError",
    };
    for node in context.nodes_of_any(&["class", "call"]) {
        // `on_send` は `csend` に呼ばれない。`Class&.new(Exception)` は本家が構造的に見ないので、
        // ここで落とさないと過剰検出になる。`class` の枝には関係しない。
        if node.kind_str() == "call" && !crate::rules::send_node::is_plain_send(node, context) {
            continue;
        }
        let Some(parent_class) = exception_reference(node, context) else {
            continue;
        };
        if node.kind_str() == "class" && shadowed_by_a_sibling_definition(node, parent_class, context) {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Inherit from `{preferred}` instead of `Exception`."),
                    parent_class.byte_range(),
                )
                .corrected_by(Edit {
                    start: parent_class.start_byte(),
                    end: parent_class.end_byte(),
                    replacement: preferred.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// The `Exception` a definition inherits from, either as a superclass or as the one argument of a
/// `Class.new`.
fn exception_reference<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    match node.kind_str() {
        "class" => {
            let parent_class = node.field("superclass")?.named_child(0)?;
            is_exception(parent_class, context).then_some(parent_class)
        }
        // `(send (const {cbase nil?} :Class) :new $(const {cbase nil?} _))`: exactly one argument,
        // and a receiver reached from the top level.
        "call" => {
            let receiver = node.field("receiver")?;
            if top_level_name(receiver, context) != Some("Class")
                || context.source.node_text(node.field("method")?) != "new"
            {
                return None;
            }
            let arguments = node.field("arguments")?;
            let argument = (arguments.named_child_count() == 1).then(|| arguments.named_child(0))??;
            (top_level_name(argument, context).is_some() && is_exception(argument, context))
                .then_some(argument)
        }
        _ => None,
    }
}

/// `const_name == 'Exception'`, which the leading `::` of `::Exception` is no part of.
fn is_exception(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    top_level_name(node, context) == Some("Exception")
}

/// The name of a constant written with no namespace or with only `::` in front of it, which is
/// what `{cbase nil?}` matches.
fn top_level_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" if node.field("scope").is_none() => {
            Some(context.source.node_text(node.field("name")?))
        }
        _ => None,
    }
}

/// `inherit_exception_class_with_omitted_namespace?`: a class defined earlier in the same body
/// under the name `Exception` is what an unqualified `Exception` then refers to. Writing `::` in
/// front says otherwise, so that spelling is still reported.
fn shadowed_by_a_sibling_definition(
    node: Node<'_>,
    parent_class: Node<'_>,
    context: &RuleContext<'_>,
) -> bool {
    if parent_class.kind_str() == "scope_resolution" {
        return false;
    }
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    let mut cursor = parent.walk();
    parent
        .named_children(&mut cursor)
        .take_while(|sibling| sibling.id() != node.id())
        .any(|sibling| {
            matches!(sibling.kind_str(), "class" | "module")
                && sibling
                    .field("name")
                    .is_some_and(|name| is_exception(name, context))
        })
}
