//! Config#for_copの旧Hash overlayとClass表示を、実本家の有限記録へ照合する。

use std::fs;
use std::process::{Command, Output};

use serde_json::Value;

fn fixture() -> Value {
    let value: Value = serde_json::from_str(include_str!("fixtures/renamed_config.json")).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(
        value["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "canonical-enabled",
            "old-naming-enabled",
            "old-style-enabled",
            "naming-disabled-over-new",
            "style-disabled-over-new",
            "old-first-enabled-over-new",
            "new-first-enabled-over-new",
            "both-style-last-enabled",
            "both-reverse-key-order",
            "both-style-last-disabled",
            "old-hash-allowed-method",
            "old-empty-hash",
            "old-ignore",
            "old-explicit-color",
            "old-force-color",
            "version",
            "verbose-version",
            "only-syntax",
            "show-new",
            "show-other",
            "list-enabled",
            "list-targets",
            "multi-file",
            "parent-old-child-new",
            "parent-old-child-old",
            "pending-inspect",
            "pending-show",
            "enabled-null",
            "except-new",
            "safe-false",
            "empty-targets",
            "old-false",
            "old-empty-array-replaces-new-allow",
            "fatal-suppresses-warning",
            "parameter-order",
            "stderr-progress"
        ]
    );
    assert_eq!(
        value["runtime_api"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "old-naming-enabled",
            "old-empty-hash",
            "pending-inspect",
            "enabled-null"
        ]
    );
    value
}

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn expected(case: &Value, key: &str, cwd: &std::path::Path) -> Vec<u8> {
    let text = String::from_utf8(bytes(case[key].as_str().unwrap())).unwrap();
    let physical;
    let cwd = if cfg!(target_os = "macos") {
        physical = fs::canonicalize(cwd).unwrap();
        physical.as_path()
    } else {
        cwd
    };
    let text = text.replace("{cwd}", &cwd.to_string_lossy().replace('\\', "/"));
    if cfg!(windows) {
        text.into_bytes()
    } else {
        text.replace("\r\n", "\n").into_bytes()
    }
}

fn run(case: &Value) -> (tempfile::TempDir, Output) {
    let directory = tempfile::tempdir().unwrap();
    for name in case["directories"].as_array().unwrap() {
        fs::create_dir(directory.path().join(name.as_str().unwrap())).unwrap();
    }
    let mut originals = Vec::new();
    for (name, text) in case["files"].as_object().unwrap() {
        let path = directory.path().join(name);
        fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
        if !case["preserved_input_paths"].is_array()
            || case["preserved_input_paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == name)
            || (name == "report.json" && case["out_preserved"] == true)
        {
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().modified().unwrap(),
            ));
        }
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory.path());
    for name in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
    ] {
        command.env_remove(name);
    }
    for (name, value) in case["env"].as_object().unwrap() {
        command.env(name, value.as_str().unwrap());
    }
    command.args(
        case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap()),
    );
    let output = command.output().unwrap();
    for (path, original, modified) in originals {
        assert_eq!(fs::read(&path).unwrap(), original, "{}", case["id"]);
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            modified,
            "{}",
            case["id"]
        );
    }
    (directory, output)
}

#[test]
fn old_hash_values_and_warning_bytes_match_the_real_cli() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["comparison"] == "json")
    {
        let (directory, output) = run(case);
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{}",
            case["id"]
        );
        assert_eq!(
            output.stderr,
            expected(case, "stderr_hex", directory.path()),
            "{}",
            case["id"]
        );
        let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut reference: Value =
            serde_json::from_slice(&bytes(case["stdout_hex"].as_str().unwrap())).unwrap();
        assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
        assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
        actual.as_object_mut().unwrap().remove("metadata");
        reference.as_object_mut().unwrap().remove("metadata");
        assert_eq!(actual, reference, "{}", case["id"]);
        count += 1;
    }
    assert_eq!(count, 27);
}

#[test]
fn catalog_and_list_modes_keep_the_measured_lazy_warning_boundaries() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap().iter().filter(|case| {
        matches!(
            case["comparison"].as_str(),
            Some("truthful_version" | "catalog_fields" | "text_transport")
        )
    }) {
        let (directory, output) = run(case);
        assert_eq!(output.status.code(), Some(0), "{}", case["id"]);
        assert_eq!(
            output.stderr,
            expected(case, "stderr_hex", directory.path()),
            "{}",
            case["id"]
        );
        let actual = String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n");
        let reference = String::from_utf8(bytes(case["stdout_hex"].as_str().unwrap()))
            .unwrap()
            .replace("\r\n", "\n");
        match case["comparison"].as_str().unwrap() {
            "truthful_version" => {
                // 非verbose入口は版だけ、verbose入口は実native metadataを表示する。
                if case["id"] == "version" {
                    assert_eq!(actual, format!("{}\n", sonicop::VERSION));
                } else {
                    assert!(actual.starts_with("sonicop "));
                }
            }
            "catalog_fields" => {
                // 基点のカタログ全項目・stdio差を今回の成功へ混ぜず、ClassのEnabledを照合する。
                assert_eq!(
                    actual.lines().find(|line| line.starts_with("  Enabled:")),
                    reference
                        .lines()
                        .find(|line| line.starts_with("  Enabled:")),
                    "{}",
                    case["id"]
                );
                let section = |text: &str| {
                    text.lines()
                        .find(|line| !line.starts_with([' ', '#']) && line.ends_with(':'))
                        .map(str::to_owned)
                };
                assert_eq!(section(&actual), section(&reference), "{}", case["id"]);
            }
            "text_transport" => assert_eq!(actual, reference, "{}", case["id"]),
            _ => unreachable!(),
        }
        count += 1;
    }
    assert_eq!(count, 7);
}

#[test]
fn fatal_validation_suppresses_migration_warnings_before_any_report() {
    let fixture = fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "fatal-suppresses-warning")
        .unwrap();
    let (directory, output) = run(case);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        expected(case, "stderr_hex", directory.path())
    );
}

#[test]
fn progress_and_using_share_the_actual_runner_event_order() {
    let fixture = fixture();
    assert_eq!(fixture["coverage"]["actual_cli"], 36);
    assert_eq!(fixture["coverage"]["actual_api"], 4);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["comparison"] == "lifecycle_bytes")
            .count(),
        1
    );
    let case = cases
        .iter()
        .find(|case| case["comparison"] == "lifecycle_bytes")
        .unwrap();
    let (directory, output) = run(case);
    assert_eq!(
        output.status.code(),
        Some(case["exit"].as_i64().unwrap() as i32)
    );
    assert_eq!(
        output.stdout,
        expected(case, "stdout_hex", directory.path())
    );
    assert_eq!(
        output.stderr,
        expected(case, "stderr_hex", directory.path())
    );
    let stderr = String::from_utf8(bytes(case["stderr_hex"].as_str().unwrap())).unwrap();
    assert!(stderr.find("Inspecting 1 file").unwrap() < stderr.find("Warning: Using").unwrap());
    assert_eq!(fixture["runtime_api"].as_array().unwrap().len(), 4);
}

#[test]
fn renamed_and_parameter_warnings_share_one_body_before_using_and_fatal_suppresses_both() {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/renamed_config_warning_order.json")).unwrap();
    assert_eq!(data["schema"], 1);
    let cases = data["cases"].as_array().unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "rename-parameter-plain",
            "rename-parameter-force-color",
            "fatal-suppresses-rename-parameter"
        ]
    );
    for case in cases {
        let mut input = case.clone();
        input["directories"] = serde_json::json!([]);
        let (directory, output) = run(&input);
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32)
        );
        assert_eq!(output.stdout, bytes(case["stdout_hex"].as_str().unwrap()));
        assert_eq!(
            output.stderr,
            expected(case, "stderr_hex", directory.path())
        );
        let actual = fs::read(directory.path().join("report.json")).unwrap();
        if case["out_preserved"] == true {
            assert_eq!(actual, bytes(case["out_hex"].as_str().unwrap()));
        } else {
            let mut actual: Value = serde_json::from_slice(&actual).unwrap();
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            let mut reference: Value =
                serde_json::from_slice(&bytes(case["out_hex"].as_str().unwrap())).unwrap();
            actual.as_object_mut().unwrap().remove("metadata");
            reference.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, reference);
        }
    }
}
