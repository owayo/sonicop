//! ParameterRuleのkey存在・Ruby下限とChangedEnforcedStylesの値条件を保つ。

use std::fmt;
use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_yaml_ng::Value;

use super::Severity;
use crate::ruby_version::{resolve_target_ruby, string_to_f};

/// ConfigValidatorが継承前に作るTargetRubyを、merge後の設定値とは別に保つ。
pub(in crate::config) struct TargetRuby {
    configured: Option<Value>,
    path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct TargetRubyGetterError(String);

impl fmt::Display for TargetRubyGetterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TargetRubyGetterError {}

impl TargetRuby {
    pub(in crate::config) fn new(config: &Value, path: Option<PathBuf>) -> Self {
        Self {
            configured: config
                .as_mapping()
                .and_then(|mapping| mapping.get("AllCops"))
                .and_then(Value::as_mapping)
                .and_then(|mapping| mapping.get("TargetRubyVersion"))
                .cloned(),
            path,
        }
    }

    fn version(&self) -> Result<f64> {
        let environment = std::env::var_os("RUBOCOP_TARGET_RUBY_VERSION")
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("RUBOCOP_TARGET_RUBY_VERSION is not valid UTF-8"))
            })
            .transpose()?;
        self.version_with_environment(environment.as_deref())
    }

    fn version_with_environment(&self, environment: Option<&str>) -> Result<f64> {
        // TargetRubyのgetterはto_f。最終supported検証用のstrict resolverを前倒ししない。
        if let Some(value) = environment {
            return Ok(string_to_f(value));
        }
        let mut configured = self.configured.as_ref();
        while let Some(Value::Tagged(tagged)) = configured {
            configured = Some(&tagged.value);
        }
        match configured {
            Some(Value::String(value)) => return Ok(string_to_f(value)),
            Some(Value::Number(value)) => {
                return value.as_f64().context("TargetRubyVersion is not a float");
            }
            Some(Value::Bool(value)) => {
                return Err(
                    TargetRubyGetterError(format!("undefined method 'to_f' for {value}")).into(),
                );
            }
            None | Some(Value::Null) => {}
            Some(_) => bail!("AllCops/TargetRubyVersion must respond to to_f"),
        }
        let cwd = std::env::current_dir()?;
        // remoteに実cache fileは無い。cwd fallbackの互換性は未計測で、local推定と分けて扱う。
        let base = super::super::loader::path_parameter_base_directory(self.path.as_deref(), &cwd);
        let inferred = resolve_target_ruby(None, base)?.version;
        Ok(string_to_f(&inferred.to_string()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Parameter,
    EnforcedStyle,
}

#[derive(Deserialize)]
struct Rule {
    kind: Kind,
    cop: String,
    parameter: String,
    severity: Severity,
    minimum_ruby_version: Option<String>,
    value: Option<String>,
    detail: String,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../config/obsolete_parameters.json"))
        .expect("recorded RuboCop parameter obsoletions")
});

impl Rule {
    fn violated(&self, config: &Value, target: impl FnOnce() -> Result<f64>) -> Result<bool> {
        // ParameterRule#violated?は値の取得より先に対象Rubyの下限を検査する。
        if let Some(minimum) = &self.minimum_ruby_version
            && !target()?.ge(&minimum
                .parse::<f64>()
                .expect("recorded minimum Ruby version"))
        {
            return Ok(false);
        }
        let Some(value) = config
            .as_mapping()
            .and_then(|mapping| mapping.get(self.cop.as_str()))
            .and_then(Value::as_mapping)
            .and_then(|mapping| mapping.get(self.parameter.as_str()))
        else {
            return Ok(false);
        };
        // false/nullもkey?を満たす。styleだけはRubyのString等値も必要になる。
        Ok(match self.kind {
            Kind::Parameter => true,
            Kind::EnforcedStyle => {
                matches!(value, Value::String(actual) if Some(actual) == self.value.as_ref())
            }
        })
    }

    fn message(&self, origin: &str) -> String {
        let subject = match self.kind {
            Kind::Parameter => format!("parameter `{}`", self.parameter),
            Kind::EnforcedStyle => format!(
                "`{}: {}`",
                self.parameter,
                self.value.as_deref().expect("recorded obsolete style")
            ),
        };
        format!(
            "obsolete {subject} (for `{}`) found in {origin}\n{}",
            self.cop, self.detail
        )
    }
}

#[derive(Default)]
pub(super) struct Messages {
    pub(super) fatal: Vec<String>,
    pub(super) warning: Vec<String>,
}

pub(super) fn messages(config: &Value, origin: &str, target: &TargetRuby) -> Result<Messages> {
    let mut messages = Messages::default();
    for rule in RULES.iter() {
        // fatalの有無によらず全rulesのviolated?を順に呼ぶ本家の評価順を保つ。
        if rule.violated(config, || target.version())? {
            let output = match rule.severity {
                Severity::Error => &mut messages.fatal,
                Severity::Warning => &mut messages.warning,
            };
            output.push(rule.message(origin));
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use serde_json::Value as Json;
    use serde_yaml_ng::{Mapping, Value};

    use super::{RULES, Severity, TargetRuby};

    fn fixture() -> Json {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/parameter_obsoletion.json"
        ))
        .unwrap()
    }

    #[test]
    fn all_sixty_rules_match_real_api_fields_and_messages_in_order() {
        let data = fixture();
        let expected = data["primary"]["rules"].as_array().unwrap();
        assert_eq!(RULES.len(), 60);
        assert_eq!(expected.len(), 60);
        let mut warnings = 0;
        let mut styles = 0;
        for (index, (rule, row)) in RULES.iter().zip(expected).enumerate() {
            assert_eq!(row["index"], index + 67);
            assert_eq!(rule.cop, row["cop"]);
            assert_eq!(rule.parameter, row["parameter"]);
            assert_eq!(rule.message("config.yml"), row["message"]);
            let minimum = row["metadata"]
                .get("minimum_ruby_version")
                .map(ToString::to_string);
            assert_eq!(rule.minimum_ruby_version, minimum);
            assert_eq!(
                rule.value.as_deref(),
                row["metadata"].get("value").and_then(Json::as_str)
            );
            assert_eq!(
                matches!(rule.kind, super::Kind::EnforcedStyle),
                row["kind"] == "ChangedEnforcedStyles"
            );
            styles += usize::from(matches!(rule.kind, super::Kind::EnforcedStyle));
            let warning = matches!(rule.severity, Severity::Warning);
            assert_eq!(warning, row["warning"]);
            warnings += usize::from(warning);
        }
        assert_eq!(styles, 2);
        assert_eq!(warnings, 30);
    }

    #[test]
    fn all_420_presence_value_style_and_ruby_probes_match_real_api() {
        let data = fixture();
        let mut count = 0;
        for (rule, row) in RULES
            .iter()
            .zip(data["primary"]["rules"].as_array().unwrap())
        {
            let probes = row["probes"].as_array().unwrap();
            assert_eq!(probes.len(), 7);
            assert_eq!(
                probes
                    .iter()
                    .map(|probe| probe["id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                [
                    "absent",
                    "false",
                    "null",
                    "matching",
                    "other",
                    "minimum_low",
                    "minimum_high"
                ]
            );
            for probe in probes {
                let mut config = Mapping::new();
                let mut section = Mapping::new();
                if probe["present"] == true {
                    section.insert(
                        Value::String(rule.parameter.clone()),
                        serde_yaml_ng::to_value(&probe["value"]).unwrap(),
                    );
                }
                config.insert(Value::String(rule.cop.clone()), Value::Mapping(section));
                let config = Value::Mapping(config);
                let original = config.clone();
                assert_eq!(
                    rule.violated(&config, || Ok(probe["target"].as_f64().unwrap()))
                        .unwrap(),
                    probe["violated"],
                    "{} {} {}",
                    rule.cop,
                    rule.parameter,
                    probe["id"]
                );
                assert_eq!(config, original);
                count += 1;
            }
        }
        assert_eq!(count, 420);
    }

    #[test]
    fn minimum_getter_uses_real_to_f_values_without_supported_version_validation() {
        let data: Json = serde_json::from_str(include_str!(
            "../../../tests/fixtures/parameter_obsoletion_getter.json"
        ))
        .unwrap();
        let cases = data["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for row in cases {
            let value = row["value"].as_str().unwrap();
            let environment = row["kind"] == "environment";
            let config = serde_yaml_ng::to_value(serde_json::json!({"AllCops": {"TargetRubyVersion": if environment { "9.9" } else { value }}})).unwrap();
            let target = TargetRuby::new(&config, None);
            assert_eq!(
                target
                    .version_with_environment(environment.then_some(value))
                    .unwrap(),
                row["target"].as_f64().unwrap()
            );
        }
    }
}
