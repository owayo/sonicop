use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Prefer keyword arguments to options hashes.";

/// `(args ... $(optarg [#suspicious_name? _] (hash)))`: a trailing `options = {}` parameter.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(suspicious) = context.setting::<Vec<String>>("SuspiciousParamNames") else {
        return;
    };
    let allowlist = context
        .setting::<Vec<String>>("Allowlist")
        .unwrap_or_default();
    // `on_args` reaches a block's parameters as well as a definition's -- upstream has one `args`
    // node for both, and asks its parent for the name.
    for node in context.nodes_of_any(&["method", "singleton_method", "block", "do_block"]) {
        let Some(list) = written_parameters(node, context) else {
            continue;
        };
        // `super_used?`: a definition that forwards to `super` cannot change its signature.
        if forwards_to_super(node) {
            continue;
        }
        if owner_name(node, context)
            .is_some_and(|name| allowlist.iter().any(|entry| entry == name))
        {
            continue;
        }
        let parameters = super::parameters::parameters(list);
        let Some(last) = parameters.last() else {
            continue;
        };
        if last.kind != "optional_parameter" {
            continue;
        }
        let (Some(name), Some(value)) = (last.name, last.value) else {
            continue;
        };
        if !suspicious
            .iter()
            .any(|entry| *entry == context.source.node_text(name))
        {
            continue;
        }
        // `(hash)`: the empty literal, with nothing in it.
        if value.kind_str() != "hash" || value.named_child_count() != 0 {
            continue;
        }
        offenses.push(context.offense(MSG, last.range.clone()));
    }
}

/// The parameter list as written, which a `->` lambda keeps on the arrow rather than on the block.
fn written_parameters<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    node.field("parameters").or_else(|| {
        context
            .parent(node)
            .filter(|parent| parent.kind_str() == "lambda")
            .and_then(|parent| parent.field("parameters"))
    })
}

/// `node.parent.method_name`: what a definition is called, or the method a block was handed to.
fn owner_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if matches!(node.kind_str(), "method" | "singleton_method") {
        return node
            .field("name")
            .map(|name| context.source.node_text(name));
    }
    let parent = context.parent(node)?;
    if parent.kind_str() == "lambda" {
        return Some("lambda");
    }
    parent
        .field("method")
        .map(|method| context.source.node_text(method))
}

/// `node.parent.each_node(:zsuper, :super).any?`, where the parent of the `args` is the definition.
fn forwards_to_super(node: Node<'_>) -> bool {
    let mut stack: Vec<Node<'_>> = super::nodes::children(node);
    while let Some(current) = stack.pop() {
        if current.kind_str() == "super" {
            return true;
        }
        stack.extend(super::nodes::children(current));
    }
    false
}
