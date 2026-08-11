use super::support::valid_name;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "snake_case".to_owned());
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name_node) = node.child_by_field_name("name") else {
            continue;
        };
        let name = context.source.node_text(name_node);
        if operator_method(name) || valid_name(name, &style) {
            continue;
        }
        offenses.push(context.offense(
            format!("Use {style} for method names."),
            name_node.byte_range(),
        ));
    }
}

/// Operators are defined with `def` too, and none of them can be spelled in the enforced style.
fn operator_method(name: &str) -> bool {
    matches!(
        name,
        "+" | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "=="
            | "==="
            | "!="
            | "<=>"
            | "<"
            | "<="
            | ">"
            | ">="
            | "[]"
            | "[]="
            | "<<"
            | ">>"
            | "&"
            | "|"
            | "^"
            | "~"
            | "`"
    )
}
