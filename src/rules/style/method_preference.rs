//! `MethodPreference#preferred_methods`, shared by the two cops that rename a selector according to
//! a `PreferredMethods` mapping.

use std::collections::HashMap;

use crate::rules::RuleContext;

/// The mapping the cop should apply.
///
/// A mapping whose *key* is the preferred name of another mapping that the configuration added is
/// dropped, which keeps a chain of renames from being applied one link at a time. Upstream tells an
/// added mapping from a shipped one by comparing against the values in the default configuration,
/// which is what `defaults` holds.
pub(super) fn preferred_methods(
    context: &RuleContext<'_>,
    defaults: &[&str],
) -> Option<HashMap<String, String>> {
    let merged = context.setting::<HashMap<String, String>>("PreferredMethods")?;
    let added: Vec<&String> = merged
        .values()
        .filter(|value| !defaults.contains(&value.as_str()))
        .collect();
    Some(
        merged
            .iter()
            .filter(|(key, _)| !added.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}
