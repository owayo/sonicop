//! 実効Enabledと検査対象の選択を分け、本家63CLIを改行styleのOS差だけ投影して照合する。
use serde_json::Value;
use std::{fs, process::Command};

fn bytes(value: &Value) -> Vec<u8> {
    let value = value.as_str().unwrap();
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn default_mode_flags_keep_explicit_cop_and_department_decisions() {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/default_cop_modes.json")).unwrap();
    run_cases(
        value,
        &[
            "disabled-true-none",
            "disabled-true-disable",
            "disabled-true-enable",
            "disabled-false-none",
            "disabled-false-disable",
            "disabled-false-enable",
            "disabled-null-none",
            "disabled-null-disable",
            "disabled-null-enable",
            "disabled-pending-none",
            "disabled-pending-disable",
            "disabled-pending-enable",
            "disabled-empty-none",
            "disabled-empty-disable",
            "disabled-empty-enable",
            "disabled-absent-none",
            "disabled-absent-disable",
            "disabled-absent-enable",
            "enabled-true-none",
            "enabled-true-disable",
            "enabled-true-enable",
            "enabled-false-none",
            "enabled-false-disable",
            "enabled-false-enable",
            "enabled-empty-none",
            "enabled-empty-disable",
            "enabled-empty-enable",
            "enabled-absent-none",
            "enabled-absent-disable",
            "enabled-absent-enable",
            "normal-true-disable",
            "normal-false-disable",
            "normal-absent-disable",
            "department-disabled-explicit-true",
            "department-enabled-empty",
            "department-enabled-absent",
            "inherit-true",
            "inherit-false",
            "inherit-null",
            "inherit-empty",
            "only-disabled",
            "only-department-disabled",
            "except-enabled",
            "pending-enabled",
            "pending-disabled",
        ],
        true,
    );
}

#[test]
fn internal_enabled_queries_keep_default_modes_separate_from_registry_selection() {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/default_cop_queries.json")).unwrap();
    run_cases(
        value,
        &[
            "ordinary",
            "enable-default",
            "enable-except-neighbor",
            "disable-only-neighbor",
            "pending-none",
            "null-none",
            "false-none",
            "pending-enable",
            "null-enable",
            "pending-disable",
            "null-disable",
            "typed-absent-none",
            "typed-pending-none",
            "typed-null-none",
            "typed-absent-enable",
        ],
        false,
    );
}

#[test]
fn enabled_lists_use_truthiness_without_only_or_except() {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/default_cop_lists.json")).unwrap();
    run_cases(
        value,
        &[
            "list-default-only",
            "list-enable-only-except",
            "list-disable-only",
        ],
        false,
    );
}

fn run_cases(fixture: Value, ids: &[&str], windows_native_style: bool) {
    assert_eq!(fixture["schema"], 1);
    assert_eq!(fixture["runtime"]["ruby"], "4.0.7");
    assert_eq!(fixture["runtime"]["gems"]["rubocop"], "1.89.0");
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    for case in cases {
        let folder = tempfile::tempdir().unwrap();
        let mut original = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let text = text.as_str().unwrap();
            assert_eq!(text.as_bytes(), bytes(&case["after_hex"][name]));
            if windows_native_style && name.ends_with(".yml") {
                let config: Value = serde_yaml_ng::from_str(text).unwrap();
                assert!(config.get("Layout/EndOfLine").is_none(), "{}", case["id"]);
            }
            // 本家nativeはOS依存。実45条件で出力不変を確認したcrlf指定だけをPOSIXへ足す。
            // Enabledを指定せず、実診断は全項目比較するため予期しない違反を除去しない。
            let text = if windows_native_style && !cfg!(windows) && name == "config.yml" {
                format!("{text}\nLayout/EndOfLine:\n  EnforcedStyle: crlf\n").into_bytes()
            } else {
                text.as_bytes().to_vec()
            };
            let path = folder.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &text).unwrap();
            let info = fs::metadata(&path).unwrap();
            original.push((
                path,
                text,
                info.modified().unwrap(),
                info.permissions().readonly(),
            ));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(folder.path())
            .env("PATH", "")
            .env("TERM", "dumb");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "CLICOLOR_FORCE",
            "NO_COLOR",
        ] {
            command.env_remove(key);
        }
        let output = command
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|v| v as i32),
            "{}",
            case["id"]
        );
        let stderr = bytes(&case["stderr_hex"]);
        let stderr = if cfg!(windows) {
            stderr
        } else {
            String::from_utf8(stderr)
                .unwrap()
                .replace("\r\n", "\n")
                .into_bytes()
        };
        assert_eq!(output.stderr, stderr, "{} stderr", case["id"]);
        if case["kind"] == "list" {
            assert_eq!(case["coverage"], "full_bytes");
            let expected = bytes(&case["stdout_hex"]);
            // Windowsは原raw全体、他OSはpipeのtext mode差だけを投影する。
            let expected = if cfg!(windows) {
                expected
            } else {
                String::from_utf8(expected)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .into_bytes()
            };
            assert_eq!(output.stdout, expected, "{}", case["id"]);
        } else {
            assert_eq!(case["coverage"], "full_report");
            let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            let mut expected = case["report"].clone();
            assert_eq!(
                expected,
                serde_json::from_slice::<Value>(&bytes(&case["stdout_hex"])).unwrap()
            );
            actual.as_object_mut().unwrap().remove("metadata");
            expected.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, expected, "{}", case["id"]);
        }
        for (path, expected_bytes, modified, readonly) in original {
            assert_eq!(fs::read(&path).unwrap(), expected_bytes, "{}", case["id"]);
            let info = fs::metadata(&path).unwrap();
            assert_eq!(info.modified().unwrap(), modified);
            assert_eq!(info.permissions().readonly(), readonly);
        }
        assert!(!folder.path().join("report.json").exists());
    }
}
