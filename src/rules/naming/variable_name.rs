use super::support::valid_name;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "snake_case".to_owned());
    let allowed: Vec<String> = context.setting("AllowedIdentifiers").unwrap_or_default();
    let forbidden: Vec<String> = context.setting("ForbiddenIdentifiers").unwrap_or_default();
    let variables = context.variable_roles();
    for node in context.nodes_of_any(&[
        "identifier",
        "instance_variable",
        "class_variable",
        "global_variable",
    ]) {
        if !variables.is_variable(node) {
            continue;
        }
        let name = context.source.node_text(node);
        let forbidden_name = forbidden.iter().any(|entry| entry == name);
        // `on_gvasgn` stops at the forbidden names: a global variable's spelling is never held
        // against the enforced style.
        if node.kind() == "global_variable" {
            if forbidden_name {
                offenses.push(forbidden_offense(context, node, name));
            }
            continue;
        }
        if allowed.iter().any(|entry| entry == name) {
            continue;
        }
        if forbidden_name {
            offenses.push(forbidden_offense(context, node, name));
        } else if !valid_name(name, &style) {
            offenses.push(context.offense(
                format!("Use {style} for variable names."),
                node.byte_range(),
            ));
        }
    }
}

fn forbidden_offense(
    context: &RuleContext<'_>,
    node: tree_sitter::Node<'_>,
    name: &str,
) -> Offense {
    context.offense(
        format!("`{name}` is forbidden, use another name instead."),
        node.byte_range(),
    )
}
