use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::ordered_gem::{self, Declaration};
use crate::rules::send_node::{arguments, is_plain_send, is_string, string_text};

use super::support::local_variables;

/// `{:add_dependency :add_runtime_dependency :add_development_dependency}`. A gemspec keeps
/// runtime and development dependencies in sections of their own, so only declarations made
/// through the same method are compared with one another.
const DEPENDENCY_METHODS: &[&str] = &[
    "add_dependency",
    "add_runtime_dependency",
    "add_development_dependency",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = local_variables(context);
    let declarations: Vec<Declaration<'_>> = context
        .nodes_of("call")
        .filter_map(|node| {
            // `(send (lvar _) {...} (str _) ...)`: the specification the dependency is added to is
            // reached through a local variable, and the gem is named by a plain string.
            if !is_plain_send(node, context)
                || !DEPENDENCY_METHODS.contains(&method_name(node, context)?)
            {
                return None;
            }
            let receiver = node.child_by_field_name("receiver")?;
            if receiver.kind() != "identifier"
                || !locals.contains(context.source.node_text(receiver))
            {
                return None;
            }
            let name = arguments(node).first()?.first();
            is_string(name, context).then(|| Declaration {
                node,
                name: string_text(name, context).to_owned(),
            })
        })
        .collect();

    ordered_gem::check(
        context,
        offenses,
        &declarations,
        &|current, previous| {
            format!(
                "Dependencies should be sorted in an alphabetical order within their section of \
                 the gemspec. Dependency `{current}` should appear before `{previous}`."
            )
        },
        &|previous, current| method_name(previous, context) == method_name(current, context),
    );
}

fn method_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    node.child_by_field_name("method")
        .map(|method| context.source.node_text(method))
}
