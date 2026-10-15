//! プロジェクト全体の呼び出しと継承を調べ、参照のない private メソッドを報告する。

use super::project_index_help as help;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const IMPLICIT: &[&str] = &[
    "initialize",
    "initialize_copy",
    "initialize_clone",
    "initialize_dup",
    "method_missing",
    "respond_to_missing?",
    "marshal_dump",
    "marshal_load",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(project) = context.project_index() else {
        return;
    };
    let literals = help::literal_names(context, true);
    project.with(|index| {
        for node in context.nodes_of("method") {
            let Some(name_node) = node.field("name") else {
                continue;
            };
            let name = context.source.node_text(name_node);
            if IMPLICIT.contains(&name) || literals.contains(name) {
                continue;
            }
            let ancestors = help::ancestors(node, context);
            if ancestors.iter().any(|ancestor| {
                matches!(
                    ancestor.kind_str(),
                    "method" | "singleton_method" | "block" | "do_block" | "singleton_class"
                )
            }) {
                continue;
            }
            let Some(namespace) = ancestors
                .iter()
                .find(|ancestor| matches!(ancestor.kind_str(), "class" | "module"))
            else {
                continue;
            };
            let Some(owner) = namespace
                .field("name")
                .and_then(|name| help::resolve_constant(index, name, context))
            else {
                continue;
            };
            if index.unused_private_method(owner, name) {
                offenses.push(context.offense(
                    format!("Private method `{name}` appears to be unused."),
                    node.start_byte()..name_node.end_byte(),
                ));
            }
        }
    });
}
