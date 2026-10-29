//! ConfigObsoletionの規則を評価し、CLIで未知copの検証より先に再生する。

use std::fmt;
use std::sync::{Arc, LazyLock};

use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};

use super::runtime::Snapshot;

mod parameters;
pub(super) use parameters::TargetRuby;
pub(crate) use parameters::TargetRubyGetterError;

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

pub(super) fn warning_name(name: &str) -> bool {
    RENAMED
        .iter()
        .any(|rule| matches!(rule.severity, Severity::Warning) && rule.old_name == name)
}

fn renamed_warnings(config: &Value, origin: &str) -> Vec<String> {
    let Some(mapping) = config.as_mapping() else {
        return Vec::new();
    };
    RENAMED
        .iter()
        .filter(|rule| matches!(rule.severity, Severity::Warning))
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

pub(super) fn legacy_settings(config: &Value) -> Vec<(&'static str, &'static str, Mapping)> {
    let disabled = config
        .get("AllCops")
        .and_then(|all| all.get("DisabledByDefault"))
        .and_then(Value::as_bool)
        == Some(true);
    RENAMED
        .iter()
        .filter(|rule| matches!(rule.severity, Severity::Warning))
        .filter_map(|rule| {
            let mut settings = config.get(&rule.old_name)?.as_mapping()?.clone();
            // ConfigLoaderResolverはDisabledByDefault時、空の旧HashにもEnabledを補う。
            if disabled && !settings.contains_key("Enabled") {
                settings.insert(Value::String("Enabled".to_owned()), Value::Bool(true));
            }
            (!settings.is_empty()).then_some((
                rule.old_name.as_str(),
                rule.new_name.as_str(),
                settings,
            ))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RetiredCop {
    Removed {
        old_name: String,
        short_name: String,
        message: String,
    },
    Split {
        old_name: String,
        short_name: String,
        message: String,
    },
    Extracted {
        old_name: String,
        department: String,
        gem: String,
    },
}

impl RetiredCop {
    fn message(&self, config: &Mapping, registry: &Snapshot) -> Option<String> {
        match self {
            Self::Removed {
                old_name,
                short_name,
                message,
            }
            | Self::Split {
                old_name,
                short_name,
                message,
            } => (config.contains_key(old_name.as_str())
                || config.contains_key(short_name.as_str()))
            .then(|| message.clone()),
            Self::Extracted {
                old_name,
                department,
                gem,
            } => {
                // 実際のloaded feature表記又はPlugin.about.nameだけを照合する。
                if registry.plugin_loaded(gem) {
                    return None;
                }
                let prefix = format!("{department}/");
                let count = config
                    .keys()
                    .filter_map(Value::as_str)
                    .filter(|name| *name == department || name.starts_with(&prefix))
                    .count();
                let subject = match count {
                    0 => return None,
                    1 => format!("`{old_name}` has"),
                    _ => format!("`{department}` cops have"),
                };
                Some(format!("{subject} been extracted to the `{gem}` gem."))
            }
        }
    }
}

static RETIRED: LazyLock<Vec<RetiredCop>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../config/obsolete_cop_categories.json"))
        .expect("recorded RuboCop removed, split and extracted cops")
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

#[cfg(test)]
pub(super) fn fatal_messages(config: &Value, origin: &str) -> Vec<String> {
    fatal_messages_with_registry(config, origin, &Snapshot::default())
}

fn fatal_messages_with_registry(config: &Value, origin: &str, registry: &Snapshot) -> Vec<String> {
    let Some(mapping) = config.as_mapping() else {
        return Vec::new();
    };
    // keyの順やEnabledの値ではなくCopRule#violated?とrulesの順を使う。
    // warningはOutcomeでparameterと合流し、ここではfatal cop規則だけを登録順に集約する。
    let mut messages = RENAMED
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
        .collect::<Vec<_>>();

    messages.extend(RETIRED.iter().filter_map(|rule| {
        rule.message(mapping, registry).map(|message| {
            format!("{message}\n(obsolete configuration found in {origin}, please update it)")
        })
    }));
    messages
}

/// quiet APIへ検証失敗を伝播せず、CLIの再生時まで評価結果を保持する。
#[derive(Debug, Default)]
pub(super) struct Outcome {
    fatal: Vec<String>,
    pub(super) warning: Option<String>,
    failure: Option<SharedFailure>,
}

#[derive(Clone, Debug)]
struct SharedFailure(Arc<anyhow::Error>);

impl fmt::Display for SharedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.0.as_ref(), formatter)
    }
}

impl std::error::Error for SharedFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // topのDisplayは既に上で出すため、同じ文面をchainへ二重に入れない。
        self.0.source()
    }
}

impl Outcome {
    pub(super) fn prepare(
        config: &Value,
        origin: &str,
        target: &TargetRuby,
        registry: &Snapshot,
    ) -> Self {
        let mut fatal = fatal_messages_with_registry(config, origin, registry);
        match parameters::messages(config, origin, target) {
            Ok(messages) => {
                fatal.extend(messages.fatal);
                let mut warnings = renamed_warnings(config, origin);
                warnings.extend(messages.warning);
                Self {
                    warning: (fatal.is_empty() && !warnings.is_empty())
                        .then(|| format!("Warning: {}", warnings.join("\n"))),
                    fatal,
                    failure: None,
                }
            }
            // 本家は全rulesのmap中に例外を出すため、先に集めたcop fatalより評価失敗が優先する。
            Err(error) => Self {
                failure: Some(SharedFailure(Arc::new(error))),
                ..Self::default()
            },
        }
    }

    pub(super) fn is_fatal(&self) -> bool {
        self.failure.is_some() || !self.fatal.is_empty()
    }

    pub(super) fn check(&self) -> anyhow::Result<()> {
        if let Some(failure) = &self.failure {
            // CLIのprefix判定に必要な型だけ保持し、それ以外は元のDisplay/source chainを再生する。
            if let Some(error) = failure.0.downcast_ref::<TargetRubyGetterError>() {
                return Err(error.clone().into());
            }
            return Err(failure.clone().into());
        }
        if !self.fatal.is_empty() {
            return Err(ObsoleteConfigError::new(&self.fatal).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ObsoleteConfigError, RENAMED, RETIRED, RetiredCop, Severity, fatal_messages};
    use serde_json::Value as Json;
    use serde_yaml_ng::{Mapping, Value};

    fn fixture() -> Json {
        serde_json::from_str(include_str!("../../tests/fixtures/config_obsoletion.json")).unwrap()
    }

    #[test]
    fn retired_table_matches_the_real_api_messages_and_category_order() {
        let fixture: Json = serde_json::from_str(include_str!(
            "../../tests/fixtures/obsolete_cop_categories.json"
        ))
        .unwrap();
        let expected = fixture["primary"]["rules"].as_array().unwrap();
        assert_eq!(RETIRED.len(), 16);
        assert_eq!(expected.len(), 16);
        for (index, (rule, expected)) in RETIRED.iter().zip(expected).enumerate() {
            assert_eq!(expected["index"], index + 51);
            assert_eq!(expected["warning"], false);
            match rule {
                RetiredCop::Removed {
                    old_name,
                    short_name,
                    message,
                }
                | RetiredCop::Split {
                    old_name,
                    short_name,
                    message,
                } => {
                    assert_eq!(old_name, expected["old_name"].as_str().unwrap());
                    assert_eq!(short_name, expected["short_name"].as_str().unwrap());
                    assert_eq!(message, expected["message"].as_str().unwrap());
                    let kind = if matches!(rule, RetiredCop::Removed { .. }) {
                        "RemovedCop"
                    } else {
                        "SplitCop"
                    };
                    assert_eq!(kind, expected["kind"]);
                }
                RetiredCop::Extracted {
                    old_name,
                    department,
                    gem,
                } => {
                    assert_eq!(expected["kind"], "ExtractedCop");
                    assert_eq!(old_name, expected["old_name"].as_str().unwrap());
                    assert_eq!(department, expected["department"].as_str().unwrap());
                    assert_eq!(gem, expected["gem"].as_str().unwrap());
                }
            }
        }
    }

    #[test]
    fn retired_presence_and_null_values_match_actual_cli_without_mutation() {
        let fixture: Json = serde_json::from_str(include_str!(
            "../../tests/fixtures/obsolete_cop_categories.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap().iter().take(16) {
            let config: Value =
                serde_yaml_ng::from_str(case["files"]["config.yml"].as_str().unwrap()).unwrap();
            let before = config.clone();
            let messages = fatal_messages(&config, "config.yml");
            let hex = case["stderr_hex"].as_str().unwrap();
            let bytes: Vec<_> = hex
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            let reference = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
            assert_eq!(
                messages.join("\n"),
                reference
                    .strip_prefix("Error: ")
                    .unwrap()
                    .trim_end_matches('\n')
            );
            assert_eq!(config, before);
        }
        for rule in RETIRED.iter() {
            let (old_name, short_name) = match rule {
                RetiredCop::Removed {
                    old_name,
                    short_name,
                    ..
                }
                | RetiredCop::Split {
                    old_name,
                    short_name,
                    ..
                } => (old_name, short_name),
                RetiredCop::Extracted { .. } => continue,
            };
            for name in [old_name, short_name] {
                // 同じbare名にLayout/Styleの両removedが該当する。実APIの全rule順で集約する。
                let expected = fixture["primary"]["rules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|reference| {
                        reference["old_name"] == *name || reference["short_name"] == *name
                    })
                    .map(|reference| {
                        format!(
                            "{}\n(obsolete configuration found in config.yml, please update it)",
                            reference["message"].as_str().unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                for value in [
                    Value::Null,
                    Value::Bool(false),
                    Value::Mapping(Mapping::new()),
                ] {
                    let config =
                        Value::Mapping(Mapping::from_iter([(Value::String(name.clone()), value)]));
                    let before = config.clone();
                    assert_eq!(fatal_messages(&config, "config.yml").join("\n"), expected);
                    assert_eq!(config, before);
                }
            }
        }
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

    #[test]
    fn quiet_retired_config_keeps_values_and_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "AllCops:\n  TargetRubyVersion: 2.7\nGemspec/DateAssignment:\n  Enabled: false\nPerformance: null\n").unwrap();
        let config = super::super::Config::load(Some(&path), directory.path()).unwrap();
        assert_eq!(
            config.cop_value::<bool>("Gemspec/DateAssignment", "Enabled"),
            Some(false)
        );
        assert!(
            config
                .user
                .as_mapping()
                .unwrap()
                .contains_key("Performance")
        );
        assert!(
            config
                .unrecognized_cop_names()
                .iter()
                .any(|name| name == "Gemspec/DateAssignment")
        );
        assert!(
            !config
                .known_cop_names()
                .any(|name| name == "Gemspec/DateAssignment")
        );
    }
}
