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
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(list) = node.field("parameters") else {
            continue;
        };
        // `super_used?`: a definition that forwards to `super` cannot change its signature.
        if forwards_to_super(node) {
            continue;
        }
        if node
            .field("name")
            .is_some_and(|name| allowlist.iter().any(|entry| *entry == context.source.node_text(name)))
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
