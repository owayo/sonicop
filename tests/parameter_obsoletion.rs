//! 本家ParameterRuleの初期拒否・warning順と色、原本/outの物理byteを照合する。

use std::{fs, process::Command};

use serde_json::Value;

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn reference_pipe(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        // Windows Rubyのpipe transportだけをhostへ投影し、原本やJSON値は変えない。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn compare_report(actual: &[u8], reference: &[u8], id: &str) {
    let mut actual: Value = serde_json::from_slice(actual).unwrap();
    let mut reference: Value = serde_json::from_slice(reference).unwrap();
    assert_eq!(actual["metadata"]["ruby_engine"], "sonicop", "{id}");
    assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0", "{id}");
    // 実装のidentity以外はsummaryと全offense fieldを残して比較する。
    actual.as_object_mut().unwrap().remove("metadata");
    reference.as_object_mut().unwrap().remove("metadata");
    assert_eq!(actual, reference, "{id}");
}

#[test]
fn parameter_errors_and_warnings_match_real_cli_bytes_and_lifecycle() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/parameter_obsoletion.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(cases.len(), 41);
    let ids: Vec<_> = cases
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "fatal-alternative",
            "fatal-present-false",
            "fatal-present-null",
            "fatal-reason",
            "fatal-rule-order",
            "fatal-suppresses-warnings",
            "warning-present-false",
            "warning-present-null",
            "warning-alternatives",
            "warning-style",
            "warning-rule-order",
            "minimum-low27",
            "minimum-low31",
            "minimum-high32",
            "minimum-high40",
            "minimum-false32",
            "minimum-null32",
            "minimum-absent32",
            "minimum-environment32",
            "canonical-style",
            "absent-parameter",
            "namespace-before-error",
            "namespace-before-warning",
            "parent-warning",
            "parent-warning-replayed",
            "child-lower-target",
            "out-fatal-preserved",
            "out-warning",
            "force-default-error",
            "force-default-warning",
            "warning-force-color",
            "warning-multiple-force-color",
            "fatal-force-color",
            "ordinary-warning-control",
            "ordinary-warning-color-control",
            "warning-color-flag",
            "warning-force-no-color",
            "minimum-inferred-gemspec",
            "minimum-inferred-version-file",
            "minimum-gemspec-precedes-version-file",
            "force-default-warning-pending-enabled"
        ]
    );
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        41
    );
    assert_eq!(
        cases
            .iter()
            .filter(|row| row["coverage"] == "strict")
            .count(),
        41
    );
    assert_eq!(
        cases
            .iter()
            .filter(|row| row["coverage"] == "prefix_157")
            .count(),
        0
    );
    let mut fatal = 0;
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().modified().unwrap(),
            ));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(directory.path()).env("PATH", "");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        );
        let actual = command.output().unwrap();
        assert_eq!(
            actual.status.code(),
            case["exit"].as_i64().map(|code| code as i32),
            "{id}"
        );
        assert_eq!(case["coverage"], "strict");
        assert_eq!(
            actual.stderr,
            reference_pipe(bytes(case["stderr_hex"].as_str().unwrap())),
            "{id}"
        );
        if id == "force-default-warning" {
            assert_eq!(case["comparison"], "all_streams");
            let prefix = reference_pipe(bytes(case["obsoletion_prefix_hex"].as_str().unwrap()));
            assert!(actual.stderr.starts_with(&prefix), "{id}");
        }
        let report_path = directory.path().join("report.json");
        assert_eq!(
            report_path.exists(),
            case["out_exists"].as_bool().unwrap(),
            "{id}"
        );
        if case["exit"] == 2 {
            fatal += 1;
            assert_eq!(
                actual.stdout,
                bytes(case["stdout_hex"].as_str().unwrap()),
                "{id}"
            );
            if report_path.exists() {
                assert_eq!(
                    fs::read(&report_path).unwrap(),
                    bytes(case["out_hex"].as_str().unwrap()),
                    "{id}"
                );
            }
        } else if report_path.exists() {
            assert_eq!(
                actual.stdout,
                bytes(case["stdout_hex"].as_str().unwrap()),
                "{id}"
            );
            compare_report(
                &fs::read(&report_path).unwrap(),
                &bytes(case["out_hex"].as_str().unwrap()),
                id,
            );
        } else {
            compare_report(
                &actual.stdout,
                &bytes(case["stdout_hex"].as_str().unwrap()),
                id,
            );
        }
        for (path, original, modified) in originals {
            assert_eq!(
                fs::read(&path).unwrap(),
                original,
                "{id}: {}",
                path.display()
            );
            assert_eq!(
                fs::metadata(&path).unwrap().modified().unwrap(),
                modified,
                "{id}: {}",
                path.display()
            );
        }
    }
    assert_eq!(fatal, 10);
}

#[test]
fn quiet_api_keeps_original_values_without_cli_validation() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/parameter_obsoletion.json")).unwrap();
    for (id, fatal) in [("warning-style", false), ("fatal-present-null", true)] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        for (name, text) in case["files"].as_object().unwrap() {
            fs::write(directory.path().join(name), text.as_str().unwrap()).unwrap();
        }
        let path = directory.path().join("config.yml");
        let loaded = sonicop::config::Config::load(Some(&path), directory.path()).unwrap();
        if fatal {
            // quiet APIは従来どおりCLI検証を行わず、nullも原値として返す。
            assert_eq!(
                loaded.cop_value::<serde_yaml_ng::Value>(
                    "Layout/CaseIndentation",
                    "IndentWhenRelativeTo"
                ),
                Some(serde_yaml_ng::Value::Null)
            );
        } else {
            // 設定の移行はしない。quiet APIへも旧値をそのまま返す。
            assert_eq!(
                loaded.cop_value::<String>("Style/EmptyClassDefinition", "EnforcedStyle"),
                Some("class_definition".to_owned())
            );
        }
    }
}

fn additional_run(case: &Value, directory: &std::path::Path) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(directory)
        .env("PATH", "")
        .env("CLICOLOR_FORCE", "0");
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(key);
    }
    for (key, value) in case["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    command.args(
        case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap()),
    );
    command.output().unwrap()
}

#[test]
fn minimum_getter_failures_preserve_priority_namespace_prefix_and_early_version() {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/parameter_obsoletion_priority.json")).unwrap();
    let cases = data["cases"].as_array().unwrap();
    assert_eq!(data["schema"], 1);
    assert_eq!(
        cases
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "bad-env-normal-fatal-0",
            "bad-env-normal-fatal-1",
            "bad-env-version-fatal-0",
            "bad-env-version-fatal-1",
            "bool-target-normal-fatal-0",
            "bool-target-normal-fatal-1",
            "bool-target-version-fatal-0",
            "bool-target-version-fatal-1",
            "parent-bool-root-valid-normal",
            "parent-bool-root-valid-version",
            "namespace-before-bool",
            "empty-env-normal",
            "empty-env-version"
        ]
    );
    let (mut early, mut getter, mut residual) = (0, 0, 0);
    for case in cases {
        match case["scope"].as_str().unwrap() {
            "existing_target_parser_lifecycle_residual" => {
                // 最終parser/版推定の既知差は保存し、最低版getterの成功件数へ混ぜない。
                residual += 1;
                continue;
            }
            "early_version_or_cop_fatal" => early += 1,
            "error_priority_prefix_envelope" => getter += 1,
            other => panic!("未計測の比較範囲: {other}"),
        }
        let id = case["id"].as_str().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text.as_str().unwrap()).unwrap();
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().modified().unwrap(),
            ));
        }
        let actual = additional_run(case, directory.path());
        assert_eq!(
            actual.status.code(),
            case["exit"].as_i64().map(|code| code as i32),
            "{id}"
        );
        let prefix = reference_pipe(bytes(case["stderr_prefix_hex"].as_str().unwrap()));
        if case["scope"] == "early_version_or_cop_fatal" {
            assert_eq!(actual.stderr, prefix, "{id}");
        } else {
            // NoMethodErrorの内部Ruby stackは比較せず、先行namespaceと先頭行まで実測する。
            assert!(
                actual.stderr.starts_with(&prefix),
                "{id}: {:?}",
                actual.stderr
            );
            assert!(!actual.stderr.contains(&0x1b), "{id}");
        }
        let stdout = if case["version"] == true && case["exit"] == 0 {
            // 製品の版identityは143の既存対照と同じ扱いで、成功phaseとnative改行を検査する。
            assert_eq!(bytes(case["stdout_hex"].as_str().unwrap()), b"1.89.0\r\n");
            reference_pipe(format!("{}\r\n", env!("CARGO_PKG_VERSION")).into_bytes())
        } else {
            reference_pipe(bytes(case["stdout_hex"].as_str().unwrap()))
        };
        assert_eq!(actual.stdout, stdout, "{id}");
        assert_eq!(
            fs::read(directory.path().join("report.json")).unwrap(),
            bytes(case["out_hex"].as_str().unwrap()),
            "{id}"
        );
        for (path, original, modified) in originals {
            assert_eq!(fs::read(&path).unwrap(), original, "{id}");
            assert_eq!(
                fs::metadata(&path).unwrap().modified().unwrap(),
                modified,
                "{id}"
            );
        }
    }
    assert_eq!((early, getter, residual), (4, 7, 2));
}

#[test]
fn minimum_getter_error_stays_plain_in_initial_and_late_color_phases() {
    let data: Value = serde_json::from_str(include_str!(
        "fixtures/parameter_obsoletion_getter_color.json"
    ))
    .unwrap();
    let cases = data["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text.as_str().unwrap()).unwrap();
            if case["preserved_input_paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == name)
            {
                originals.push((
                    path.clone(),
                    fs::read(&path).unwrap(),
                    fs::metadata(&path).unwrap().modified().unwrap(),
                ));
            }
        }
        let actual = additional_run(case, directory.path());
        assert_eq!(actual.status.code(), Some(2));
        let prefix = reference_pipe(bytes(case["stderr_prefix_hex"].as_str().unwrap()));
        assert!(actual.stderr.starts_with(&prefix));
        assert!(!actual.stderr.contains(&0x1b));
        assert!(actual.stdout.is_empty());
        if case["out_preserved"] == true {
            assert_eq!(
                fs::read(directory.path().join("report.json")).unwrap(),
                bytes(case["out_before_hex"].as_str().unwrap())
            );
        }
        // lateのerrorJSON/out lifecycleは#152の残差。ここではplain先頭行と原本だけを比較する。
        for (path, original, modified) in originals {
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        }
    }
}

#[test]
fn quiet_api_does_not_propagate_the_inherited_minimum_getter_failure() {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/parameter_obsoletion_priority.json")).unwrap();
    let case = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "parent-bool-root-valid-normal")
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in case["files"].as_object().unwrap() {
        fs::write(directory.path().join(name), text.as_str().unwrap()).unwrap();
    }
    let path = directory.path().join("config.yml");
    // quiet APIは従来の最終値解決だけを行う。親のCLI検証失敗を新たに伝播しない。
    let loaded = sonicop::config::Config::load(Some(&path), directory.path()).unwrap();
    assert_eq!(
        loaded.cop_value::<f64>("AllCops", "TargetRubyVersion"),
        Some(2.7)
    );
}
