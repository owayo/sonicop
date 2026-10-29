//! ConfigValidatorの未知節判定を、meta/部門の一覧や通常のcop選択と混ぜない。

use serde_yaml_ng::Value;
use std::fmt;

use super::namespaces::{registered_or_obsolete, suggestion_dictionary};
use super::runtime::Snapshot;

#[derive(Debug)]
pub(crate) struct UnknownConfigError {
    message: String,
    color: bool,
}

impl UnknownConfigError {
    pub(super) fn new(messages: &[String]) -> Self {
        Self {
            message: messages.join("\n"),
            color: false,
        }
    }

    pub(crate) fn color(&self) -> bool {
        self.color
    }

    pub(crate) fn with_color(mut error: anyhow::Error, color: bool) -> anyhow::Error {
        if let Some(unknown) = error.downcast_mut::<Self>() {
            unknown.color = color;
        }
        error
    }
}

impl fmt::Display for UnknownConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UnknownConfigError {}

#[cfg(test)]
pub(super) fn names(value: &Value) -> Vec<String> {
    names_with_registry(value, &Snapshot::default())
}

pub(super) fn names_with_registry(value: &Value, registry: &Snapshot) -> Vec<String> {
    value
        .as_mapping()
        .into_iter()
        .flat_map(|mapping| mapping.keys())
        .filter_map(Value::as_str)
        .filter(|name| {
            !matches!(
                *name,
                "AllCops" | "inherit_mode" | "inherit_from" | "inherit_gem" | "require" | "plugins"
            )
        })
        .filter(|name| {
            !registered_or_obsolete(name)
                && !registry.names.iter().any(|registered| registered == name)
                && !registry
                    .departments
                    .iter()
                    .any(|department| department == name)
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
pub(super) fn messages(names: &[String], display: &str) -> Vec<String> {
    messages_with_registry(names, display, &Snapshot::default())
}

pub(super) fn messages_with_registry(
    names: &[String],
    display: &str,
    registry: &Snapshot,
) -> Vec<String> {
    names
        .iter()
        .map(|name| {
            let dictionary: Vec<&str> = if registry.names.is_empty() {
                suggestion_dictionary().collect()
            } else {
                registry
                    .departments
                    .iter()
                    .chain(&registry.names)
                    .map(String::as_str)
                    .collect()
            };
            let mut message = format!("unrecognized cop or department {name} found in {display}");
            let suggestions = crate::name_distance::similar_names(name, dictionary);
            if !suggestions.is_empty() {
                message.push_str(&format!("\nDid you mean `{}`?", suggestions.join("`, `")));
            }
            message
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/unknown_config_cops.json"
        ))
        .unwrap()
    }

    #[test]
    fn catalogs_and_suggestion_order_match_the_real_registry() {
        let fixture = reference();
        let api = &fixture["api"];
        for key in ["registry", "departments", "legacy"] {
            for name in api[key].as_array().unwrap() {
                assert!(registered_or_obsolete(name.as_str().unwrap()), "{name}");
            }
        }
        let expected: Vec<_> = api["departments"]
            .as_array()
            .unwrap()
            .iter()
            .chain(api["registry"].as_array().unwrap())
            .map(|name| name.as_str().unwrap())
            .collect();
        assert_eq!(suggestion_dictionary().collect::<Vec<_>>(), expected);
        let mut mapping = serde_yaml_ng::Mapping::new();
        for key in api["default_keys"].as_array().unwrap() {
            mapping.insert(Value::String(key.as_str().unwrap().to_owned()), Value::Null);
        }
        assert!(names(&Value::Mapping(mapping)).is_empty());
    }

    #[test]
    fn quiet_config_keeps_bare_unknown_metadata_out_of_known_cops() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(
            &path,
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\nNoSuchCop: {}\nLayout/TrailingBlankLines:\n  Enabled: false\n",
        )
        .unwrap();
        let config = super::super::Config::load(Some(&path), directory.path()).unwrap();
        // libraryのmetadata契約を保ち、CLIのfatalと値解決を混同しない。
        assert_eq!(
            config.unrecognized_cop_names(),
            ["Layout/TrailingBlankLines", "NoSuchCop"]
        );
        assert!(!config.known_cop_names().any(|name| name == "NoSuchCop"));
    }

    #[test]
    fn typed_messages_and_context_color_keep_the_upstream_suggestions() {
        let fixture = reference();
        for id in [
            "unknown-map",
            "unknown-unicode",
            "typo-department",
            "typo-full",
            "known-snake-full",
        ] {
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap();
            let value: Value =
                serde_yaml_ng::from_str(case["files"]["config.yml"].as_str().unwrap()).unwrap();
            let messages = messages(&names(&value), "config.yml");
            let error: anyhow::Error = UnknownConfigError::new(&messages).into();
            let body = format!("Error: {error}\n").replace('\n', "\r\n");
            let hex: String = body
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(hex, case["stderr_hex"], "{id}");
            let colored = UnknownConfigError::with_color(error.context("outer context"), true);
            assert_eq!(colored.to_string(), "outer context");
            assert!(
                colored
                    .downcast_ref::<UnknownConfigError>()
                    .unwrap()
                    .color()
            );
        }
    }
}
