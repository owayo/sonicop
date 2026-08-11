use regex::Regex;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Use CamelCase for classes and modules.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedNames").unwrap_or_default();
    // The names are joined into one alternation and stripped out of the constant path before the
    // underscores are counted, so `module_parent::MyModule` passes while `module_parent::My_Module`
    // does not. An empty list yields an empty pattern, which matches nothing away.
    let permitted = Regex::new(&allowed.join("|")).ok();

    for node in context.nodes_of_any(&["class", "module"]) {
        let Some(name_node) = node.child_by_field_name("name") else {
            continue;
        };
        let name = context.source.node_text(name_node);
        if !name.contains('_') {
            continue;
        }
        let stripped = permitted.as_ref().map_or_else(
            || name.to_owned(),
            |pattern| pattern.replace_all(name, "").into_owned(),
        );
        if stripped.contains('_') {
            offenses.push(context.offense(MSG, name_node.byte_range()));
        }
    }
}
