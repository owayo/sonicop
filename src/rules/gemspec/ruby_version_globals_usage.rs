use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // Upstream guards this with `gem_specification(processed_source.ast) && ruby_version?(node)`,
    // but a `def_node_search` called without a block returns an enumerator, which is truthy even
    // when it finds nothing. Every `RUBY_VERSION` in a gemspec is reported, block or no block.
    for node in context.nodes_of_any(&["constant", "scope_resolution"]) {
        if !ruby_version(node, context) {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "Do not use `{}` in gemspec file.",
                context.source.node_text(node)
            ),
            node.byte_range(),
        ));
    }
}

/// `{(const {cbase nil?} :RUBY_VERSION) (const (const {cbase nil?} :Ruby) :VERSION)}`.
///
/// The constants inside `::RUBY_VERSION` and `Ruby::VERSION` are nodes of their own here where
/// upstream has a single `const`, so a constant that a scope resolution owns is left to the
/// resolution around it rather than reported twice.
fn ruby_version(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => {
            node.parent()
                .is_none_or(|parent| parent.kind_str() != "scope_resolution")
                && context.source.node_text(node) == "RUBY_VERSION"
        }
        "scope_resolution" => {
            let Some(name) = node.field("name") else {
                return false;
            };
            let name = context.source.node_text(name);
            let scope = node.field("scope");
            match scope {
                None => name == "RUBY_VERSION",
                Some(scope) => {
                    name == "VERSION"
                        && crate::rules::send_node::top_level_constant(scope, "Ruby", context)
                }
            }
        }
        _ => false,
    }
}
