//! ConfigValidatorの未知節拒否とignoreの警告順を、実CLIの記録で照合する。

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::time::SystemTime;

use serde_json::Value;

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|part| u8::from_str_radix(std::str::from_utf8(part).unwrap(), 16).unwrap())
        .collect()
}

fn stderr_bytes(hex: &str) -> Vec<u8> {
    let raw = bytes(hex);
    if cfg!(windows) {
        raw
    } else {
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn inspect(case: &Value) {
    let root = tempfile::tempdir().unwrap();
    let mut before = BTreeMap::<String, (Vec<u8>, SystemTime)>::new();
    for (path, text) in case["files"].as_object().unwrap() {
        let data = text.as_str().unwrap().as_bytes();
        fs::create_dir_all(root.path().join(path).parent().unwrap()).unwrap();
        fs::write(root.path().join(path), data).unwrap();
        before.insert(
            path.clone(),
            (
                data.to_vec(),
                fs::metadata(root.path().join(path))
                    .unwrap()
                    .modified()
                    .unwrap(),
            ),
        );
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(root.path());
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
    let result = command.output().unwrap();
    let id = case["id"].as_str().unwrap();
    assert_eq!(
        root.path().join("report.json").exists(),
        case["out_exists"].as_bool().unwrap(),
        "{id}"
    );
    assert_eq!(
        result.status.code().map(i64::from),
        case["exit"].as_i64(),
        "{id}"
    );
    #[cfg(windows)]
    let display_root = root.path().to_owned();
    #[cfg(not(windows))]
    let display_root = fs::canonicalize(root.path()).unwrap();
    let expected = String::from_utf8(stderr_bytes(case["stderr_hex"].as_str().unwrap())).unwrap();
    assert_eq!(
        result.stderr,
        expected
            .replace("<ROOT>", &display_root.to_string_lossy().replace('\\', "/"))
            .as_bytes(),
        "{id}"
    );
    if case["exit"] == 2 {
        assert!(result.stdout.is_empty(), "{id}");
    } else {
        let mut report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
        assert_eq!(report["metadata"]["ruby_engine"], "sonicop");
        report.as_object_mut().unwrap().remove("metadata");
        assert_eq!(report, case["report"], "{id}");
    }
    for (path, (data, modified)) in before {
        assert_eq!(
            fs::read(root.path().join(&path)).unwrap(),
            data,
            "{id}/{path}"
        );
        assert_eq!(
            fs::metadata(root.path().join(&path))
                .unwrap()
                .modified()
                .unwrap(),
            modified,
            "{id}/{path}"
        );
    }
}

#[test]
fn unknown_config_sections_match_the_real_cli() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/unknown_config_cops.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 42);
    assert_eq!(fixture["schema"], 1);
    let ids: std::collections::HashSet<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 42);
    let order: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        [
            "unknown-map",
            "unknown-disabled",
            "unknown-empty-map",
            "unknown-null",
            "unknown-scalar",
            "unknown-full",
            "two-unknown",
            "unknown-lower",
            "unknown-unicode",
            "typo-department",
            "typo-full",
            "root-layout",
            "known-short",
            "known-full",
            "known-snake-short",
            "known-snake-full",
            "reserved-inherit-mode",
            "reserved-inherit-from",
            "reserved-inherit-gem",
            "reserved-require",
            "reserved-plugins",
            "all-cops-parameter",
            "all-cops-case",
            "wrong-namespace",
            "unknown-with-short",
            "unknown-with-bad-parameter",
            "ignore",
            "ignore-color",
            "ignore-two-color",
            "ignore-with-bad-parameter",
            "reject-color",
            "reject-out",
            "reject-version",
            "reject-verbose-version",
            "reject-force-default",
            "inherited-unknown",
            "inherited-overridden",
            "ignore-force",
            "ignore-force-no-color",
            "reject-force",
            "legacy-disabled",
            "legacy-ignore"
        ]
    );
    assert_eq!(
        fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["coverage"] == "strict")
            .count(),
        40
    );
    assert_eq!(
        fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["coverage"] == "deferred_145")
            .count(),
        2
    );
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "strict")
        .collect();
    assert_eq!(cases.len(), 40);
    for case in cases {
        inspect(case);
    }
}

#[test]
fn obsolete_names_keep_their_raw_reference_for_the_deferred_issue() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/unknown_config_cops.json")).unwrap();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["id"].as_str().unwrap().starts_with("legacy-"))
        .collect();
    assert_eq!(cases.len(), 2);
    for case in cases {
        // #145は未実装。元の本家rawを残し、generic拒否を新たな一致期待へ書き換えない。
        assert_eq!(case["coverage"], "deferred_145");
        assert_eq!(case["exit"], 2);
        assert!(case["stdout_hex"].as_str().unwrap().is_empty());
        assert!(!case["stderr_hex"].as_str().unwrap().is_empty());
    }
}

#[test]
fn inherited_unknown_warnings_replay_each_edge_and_survive_a_missing_file() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/unknown_config_trace.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    let ids: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "diamond-ignore-force-0",
            "diamond-ignore-force-1",
            "warning-before-missing-force-0",
            "warning-before-missing-force-1"
        ]
    );
    for original in cases {
        let mut case = original.clone();
        case["out_exists"] = Value::Bool(false);
        case["stderr_hex"] = case["stderr_template_hex"].clone();
        if let Some(report) = case.get_mut("report").and_then(Value::as_object_mut) {
            report.remove("metadata");
        }
        inspect(&case);
    }
}
