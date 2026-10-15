//! 定義の YARD タグと参照を、本家と共通のプロジェクト索引で結び付ける。

use super::project_index_help as help;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(project) = context.project_index() else {
        return;
    };
    project.with(|index| {
        for node in context.nodes_of_any(&["constant", "scope_resolution"]) {
            if !help::constant_reference(node, context)
                || help::within_deprecated_definition(node, context)
            {
                continue;
            }
            let Some(declaration) = help::resolve_constant(index, node, context) else {
                continue;
            };
            if let Some(detail) = index.deprecated_detail(declaration)
                && let Some(name) = help::constant_name(node, context)
            {
                offenses.push(context.offense(
                    format!("Constant `{name}` is deprecated{detail}"),
                    node.byte_range(),
                ));
            }
        }
        for node in
            context.nodes_of_any(&["call", "identifier", "element_reference", "binary", "unary"])
        {
            let Some(call) = help::method_call(node, context) else {
                continue;
            };
            let name = call.name;
            let member = format!("{name}()");
            let declaration = match call.receiver {
                Some(receiver) if receiver.kind_str() != "self" => {
                    help::resolve_constant(index, receiver, context)
                        .and_then(|owner| index.singleton_member(owner, &member))
                }
                _ => help::enclosing_namespace(index, node, context).and_then(|owner| {
                    if help::singleton_context(node, context) {
                        index.singleton_member(owner, &member)
                    } else {
                        index.find_member(owner, &member)
                    }
                }),
            };
            if let Some(declaration) = declaration
                && let Some(detail) = index.deprecated_detail(declaration)
                && !help::within_deprecated_definition(node, context)
            {
                offenses.push(context.offense(
                    format!("Method `{name}` is deprecated{detail}"),
                    call.selector,
                ));
            }
        }
    });
}
