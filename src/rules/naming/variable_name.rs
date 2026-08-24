use super::support::valid_name;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "snake_case".to_owned());
    let allowed: Vec<String> = context.setting("AllowedIdentifiers").unwrap_or_default();
    let forbidden: Vec<String> = context.setting("ForbiddenIdentifiers").unwrap_or_default();
    // `forbidden_name?` is `forbidden_identifier? || forbidden_pattern?`: a name may be forbidden
    // by an exact spelling or by a pattern it matches anywhere.
    let patterns = super::support::forbidden_patterns(context);
    // `valid_name?` is `super || matches_allowed_pattern?(name)`: the allowed patterns excuse a
    // name from the **enforced style** only. A forbidden name stays forbidden however it is spelt.
    let allowed_patterns = super::support::forbidden_patterns_named(context, "AllowedPatterns");
    let variables = context.variable_analysis();
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
        // `SIGILS = '@$'`: both `allowed_identifier?` and `forbidden_identifier?` strip them before
        // matching, so `AllowedIdentifiers: [first_arg]` covers `@first_arg` too. **The patterns do
        // not** -- `forbidden_pattern?` is given the name as written.
        let bare = name.replace(['@', '$'], "");
        let forbidden_name = forbidden.contains(&bare)
            || patterns.iter().any(|pattern| pattern.is_match(name));
        // `on_gvasgn` stops at the forbidden names: a global variable's spelling is never held
        // against the enforced style.
        if node.kind_str() == "global_variable" {
            if forbidden_name {
                offenses.push(forbidden_offense(context, node, name));
            }
            continue;
        }
        if allowed.contains(&bare) {
            continue;
        }
        if forbidden_name {
            offenses.push(forbidden_offense(context, node, name));
        } else if !valid_name(name, &style)
            && !allowed_patterns.iter().any(|pattern| pattern.is_match(name))
        {
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
