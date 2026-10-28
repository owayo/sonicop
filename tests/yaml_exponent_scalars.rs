//! 設定の型を直接照合し、表示側の変換で読込時の誤りを隠さない。

use std::fs;
use std::process::Command;

use serde_json::{Value, json};
use serde_yaml_ng::Value as YamlValue;

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/yaml_exponent_scalars.json")).unwrap();
    // 本家の有限記録が欠けても、空のループで成功してはいけない。
    assert_eq!(fixture["schema"], 1);
    for (group, count) in [("types", 23), ("behaviors", 3)] {
        let cases = fixture[group].as_array().unwrap();
        assert_eq!(cases.len(), count);
        let ids: std::collections::HashSet<_> = cases
            .iter()
            .map(|case| case["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), count);
    }
    let fields: usize = fixture["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["expected"].as_object().unwrap().len())
        .sum();
    assert_eq!(fields, 25);
    fixture
}

fn typed_value(value: YamlValue) -> Value {
    match value {
        YamlValue::String(value) => json!({"type": "String", "value": value}),
        YamlValue::Number(value) if value.is_f64() => {
            json!({"type": "Float", "value": value.as_f64().unwrap()})
        }
        YamlValue::Number(value) => {
            json!({"type": "Integer", "value": value.as_i64().unwrap()})
        }
        other => panic!("本家にない型: {other:?}"),
    }
}

#[test]
fn configuration_exponent_types_match_real_config_loader() {
    let mut differences = Vec::new();
    for case in fixture()["types"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        let original = case["config"].as_str().unwrap().as_bytes();
        fs::write(&path, original).unwrap();
        let config = sonicop::config::Config::load(Some(&path), directory.path()).unwrap();
        for (key, expected) in case["expected"].as_object().unwrap() {
            let value = config
                .cop_value::<YamlValue>("Lint/BigDecimalNew", key)
                .unwrap();
            let actual = typed_value(value);
            if actual != *expected {
                differences.push(
                    json!({"case": case["id"], "key": key, "expected": expected, "actual": actual}),
                );
            }
        }
        assert_eq!(fs::read(path).unwrap(), original);
    }
    assert!(differences.is_empty(), "型の差: {differences:#?}");
}

#[test]
fn allowed_patterns_use_the_reference_string_values() {
    for case in fixture()["behaviors"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(
            directory.path().join("case.rb"),
            case["source"].as_str().unwrap(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(directory.path());
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        for arg in case["args"].as_array().unwrap() {
            command.arg(arg.as_str().unwrap());
        }
        let result = command.output().unwrap();
        assert_eq!(
            result.status.code(),
            case["exit"].as_i64().map(|value| value as i32),
            "{}",
            case["id"]
        );
        assert!(
            result.stderr.is_empty(),
            "{}: {:?}",
            case["id"],
            result.stderr
        );
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["files"], case["report"]["files"], "{}", case["id"]);
        assert_eq!(
            report["summary"], case["report"]["summary"],
            "{}",
            case["id"]
        );
        assert_eq!(
            fs::read(directory.path().join("config.yml")).unwrap(),
            case["config"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            fs::read(directory.path().join("case.rb")).unwrap(),
            case["source"].as_str().unwrap().as_bytes()
        );
    }
}
