use std::collections::HashMap;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The values of `PreferredMethods` in the bundled default configuration, which upstream reads back
/// through `default_cop_config` to tell an added mapping from a shipped one.
const DEFAULT_PREFERENCES: [&str; 1] = ["to_sym"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(preferences) = preferred_methods(context) else {
        return;
    };
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let current = context.source.node_text(selector);
        let Some(prefer) = preferences.get(current) else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    format!("Prefer `{prefer}` over `{current}`."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: prefer.clone(),
                    safe: true,
                }),
        );
    }
}

/// `MethodPreference#preferred_methods`.
///
/// A mapping whose *key* is the preferred name of another mapping that the configuration added is
/// dropped, which keeps a chain of renames from being applied one link at a time.
fn preferred_methods(context: &RuleContext<'_>) -> Option<HashMap<String, String>> {
    let merged = context.setting::<HashMap<String, String>>("PreferredMethods")?;
    let added: Vec<&String> = merged
        .values()
        .filter(|value| !DEFAULT_PREFERENCES.contains(&value.as_str()))
        .collect();
    Some(
        merged
            .iter()
            .filter(|(key, _)| !added.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}
