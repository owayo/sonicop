//! `Style/OptionalBooleanParameter`: a parameter defaulting to `true` or `false` reads as a flag
//! at the call site, where a keyword would name what it switches.

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    for definition in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name) = definition.field("name") else {
            continue;
        };
        if allowed
            .iter()
            .any(|method| method == context.source.node_text(name))
        {
            continue;
        }
        let Some(list) = definition.field("parameters") else {
            continue;
        };
        for parameter in super::parameters::parameters(list) {
            if parameter.kind != "optional_parameter" {
                continue;
            }
            let (Some(name), Some(value)) = (parameter.name, parameter.value) else {
                continue;
            };
            if !matches!(value.kind_str(), "true" | "false") {
                continue;
            }
            let message = format!(
                "Prefer keyword arguments for arguments with a boolean default value; use \
                 `{}: {}` instead of `{}`.",
                context.source.node_text(name),
                context.source.node_text(value),
                context.source.slice(parameter.range.clone()),
            );
            offenses.push(context.offense(message, parameter.range));
        }
    }
}
