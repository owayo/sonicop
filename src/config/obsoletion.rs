//! ConfigObsoletionのfatal renameだけを、未知copの検証より先に処理する。

use std::fmt;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_yaml_ng::Value;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Severity {
    Error,
    Warning,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Renamed,
    Moved,
}

#[derive(Deserialize)]
struct RenamedCop {
    old_name: String,
    short_name: String,
    new_name: String,
    severity: Severity,
    action: Action,
}

impl RenamedCop {
    fn message(&self) -> String {
        let verb = match self.action {
            Action::Renamed => "renamed",
            Action::Moved => "moved",
        };
        format!(
            "The `{}` cop has been {verb} to `{}`.",
            self.old_name, self.new_name
        )
    }
}

static RENAMED: LazyLock<Vec<RenamedCop>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../config/renamed_cops.json"))
        .expect("recorded RuboCop renamed cops")
});

#[derive(Debug)]
pub(crate) struct ObsoleteConfigError {
    message: String,
    color: bool,
}

impl fmt::Display for ObsoleteConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ObsoleteConfigError {}

impl ObsoleteConfigError {
    pub(super) fn new(messages: &[String]) -> Self {
        Self {
            message: messages.join("\n"),
            color: false,
        }
    }

    pub(crate) fn with_color(mut error: anyhow::Error, color: bool) -> anyhow::Error {
        if let Some(obsolete) = error.downcast_mut::<Self>() {
            obsolete.color = color;
        }
        error
    }

    pub(crate) fn color(&self) -> bool {
        self.color
    }
}

pub(super) fn fatal_messages(config: &Value, origin: &str) -> Vec<String> {
    let Some(mapping) = config.as_mapping() else {
        return Vec::new();
    };
    // keyの順やEnabledの値ではなくCopRule#violated?とrulesの順を使う。
    // warning2の旧設定mergeやremoved/split/extracted/parameterはこの検査へ混ぜない。
    RENAMED
        .iter()
        .filter(|rule| matches!(rule.severity, Severity::Error))
        .filter(|rule| {
            mapping.contains_key(rule.old_name.as_str())
                || mapping.contains_key(rule.short_name.as_str())
        })
        .map(|rule| {
            format!(
                "{}\n(obsolete configuration found in {origin}, please update it)",
                rule.message()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ObsoleteConfigError, RENAMED, Severity, fatal_messages};
    use serde_json::Value as Json;
    use serde_yaml_ng::{Mapping, Value};

    fn fixture() -> Json {
        serde_json::from_str(include_str!("../../tests/fixtures/config_obsoletion.json")).unwrap()
    }

    #[test]
    fn renamed_table_matches_every_real_api_rule_in_order() {
        let fixture = fixture();
        let expected = fixture["primary"]["rules"].as_array().unwrap();
        assert_eq!(RENAMED.len(), 51);
        assert_eq!(expected.len(), 51);
        let mut fatal = 0;
        let mut warning = 0;
        for (rule, expected) in RENAMED.iter().zip(expected) {
            assert_eq!(rule.old_name, expected["old_name"]);
            assert_eq!(rule.short_name, expected["short_name"]);
            assert_eq!(rule.new_name, expected["new_name"]);
            assert_eq!(rule.message(), expected["message"]);
            let is_warning = matches!(rule.severity, Severity::Warning);
            assert_eq!(is_warning, expected["warning"]);
            if is_warning {
                warning += 1;
            } else {
                fatal += 1;
            }
        }
        assert_eq!((fatal, warning), (49, 2));
    }

    #[test]
    fn fatal_presence_uses_full_or_short_name_without_mutating_values() {
        let fixture = fixture();
        for (rule, expected) in RENAMED
            .iter()
            .zip(fixture["primary"]["rules"].as_array().unwrap())
        {
            for name in [&rule.old_name, &rule.short_name] {
                for value in [
                    Value::Null,
                    Value::Bool(false),
                    Value::Bool(true),
                    Value::String("value".to_owned()),
                    Value::Mapping(Mapping::new()),
                ] {
                    let input =
                        Value::Mapping(Mapping::from_iter([(Value::String(name.clone()), value)]));
                    let before = input.clone();
                    let result = fatal_messages(&input, "config.yml");
                    assert_eq!(input, before);
                    if matches!(rule.severity, Severity::Warning) {
                        // warningはfatalへ格上げせず、旧→新への自動書換も行わない。
                        assert!(result.is_empty(), "{name}");
                    } else {
                        let text = result.join("\n");
                        assert!(
                            text.contains(expected["message"].as_str().unwrap()),
                            "{name}: {text}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn phase_color_only_changes_the_typed_error_data() {
        let config: Value = serde_yaml_ng::from_str("Layout/TrailingBlankLines: {}\n").unwrap();
        let error: anyhow::Error =
            ObsoleteConfigError::new(&fatal_messages(&config, "config.yml")).into();
        let before = error.to_string();
        assert!(!error.downcast_ref::<ObsoleteConfigError>().unwrap().color());
        let error = ObsoleteConfigError::with_color(error.context("outer context"), true);
        assert!(error.downcast_ref::<ObsoleteConfigError>().unwrap().color());
        assert_eq!(error.to_string(), "outer context");
        assert_eq!(
            error
                .downcast_ref::<ObsoleteConfigError>()
                .unwrap()
                .to_string(),
            before
        );
        let other = ObsoleteConfigError::with_color(anyhow::anyhow!("general error"), true);
        assert!(other.downcast_ref::<ObsoleteConfigError>().is_none());
        assert_eq!(other.to_string(), "general error");
    }

    #[test]
    fn library_load_keeps_the_existing_unknown_metadata_contract() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(
            &path,
            "AllCops:\n  TargetRubyVersion: 2.7\nLayout/TrailingBlankLines:\n  Enabled: false\n",
        )
        .unwrap();
        let config = super::super::Config::load(Some(&path), directory.path()).unwrap();
        // 初期CLI検証とlibraryのmetadata APIを混ぜず、旧節を自動移行しない。
        assert!(
            config
                .unrecognized_cop_names()
                .iter()
                .any(|name| name == "Layout/TrailingBlankLines")
        );
        assert_eq!(
            config.cop_value::<bool>("Layout/TrailingBlankLines", "Enabled"),
            Some(false)
        );
        assert!(
            config
                .user
                .as_mapping()
                .unwrap()
                .contains_key("Layout/TrailingBlankLines")
        );
    }
}
