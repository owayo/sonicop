//! CRLF 保存形式と Ruby 文字列値を区別し、本家の全診断・補正 bytes と照合する。
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    id: String,
    path: String,
    source_hex: String,
    offenses: Vec<Value>,
    after_hex: String,
    raw_reference_after_hex: String,
    before_value: Value,
    after_value: Value,
}
#[derive(Deserialize)]
struct Group {
    target: String,
    cop: String,
    style: String,
    consistent: bool,
    ending: String,
    mode: String,
    config: String,
    exit: i32,
    summary: Value,
    files: Vec<Case>,
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

#[test]
fn crlf_string_literal_corrections_preserve_the_runtime_value() {
    let groups: Vec<Group> =
        serde_json::from_str(include_str!("fixtures/string_literal_crlf.json")).unwrap();
    assert_eq!(groups.len(), 72);
    assert_eq!(
        groups.iter().map(|group| group.files.len()).sum::<usize>(),
        4248
    );
    let mut failures = Vec::new();
    let mut targets = BTreeSet::new();
    let mut cops = BTreeSet::new();
    let mut headers = BTreeSet::new();
    for group in groups {
        targets.insert(group.target.clone());
        cops.insert(group.cop.clone());
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(".rubocop.yml"), &group.config).unwrap();
        for case in &group.files {
            headers.insert(case.id.split('/').next().unwrap().to_owned());
            fs::write(folder.path().join(&case.path), bytes(&case.source_hex)).unwrap();
            // 本家を同じ MRI 2.7.8 で実行した記録。値 bytes と encoding が不変だった条件だけ。
            assert_eq!(
                case.before_value, case.after_value,
                "{} / {}",
                group.target, case.id
            );
            assert!(!case.raw_reference_after_hex.is_empty());
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(folder.path()).args([
            "--cache",
            "false",
            "--no-parallel",
            "--format",
            "json",
            "--only",
            &group.cop,
        ]);
        if group.mode != "inspect" {
            command.arg(&group.mode);
        }
        let output = command
            .args(group.files.iter().map(|case| &case.path))
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let label = format!(
            "{}/{}/{}/{}/{}/{}",
            group.target, group.cop, group.style, group.consistent, group.ending, group.mode
        );
        assert_eq!(
            report["summary"]["target_file_count"],
            group.files.len(),
            "{label}"
        );
        assert_eq!(
            report["summary"]["inspected_file_count"],
            group.files.len(),
            "{label}"
        );
        let raw_files = report["files"].as_array().unwrap();
        assert_eq!(raw_files.len(), group.files.len(), "{label}");
        let files: HashMap<_, _> = raw_files
            .iter()
            .map(|file| (file["path"].as_str().unwrap(), &file["offenses"]))
            .collect();
        assert_eq!(files.len(), group.files.len(), "{label}");
        if report["summary"] != group.summary || output.status.code() != Some(group.exit) {
            failures.push(format!("{label}: summary/exit"));
        }
        for case in &group.files {
            let mut actual: Vec<Value> =
                serde_json::from_value((**files.get(case.path.as_str()).unwrap()).clone()).unwrap();
            actual.sort_by_cached_key(Value::to_string);
            let mut expected = case.offenses.clone();
            expected.sort_by_cached_key(Value::to_string);
            let actual_bytes = fs::read(folder.path().join(&case.path)).unwrap();
            if actual != expected || actual_bytes != bytes(&case.after_hex) {
                failures.push(format!(
                    "{label}/{}: offenses {} / {}; bytes {} / {}",
                    case.id,
                    serde_json::to_string(&actual).unwrap(),
                    serde_json::to_string(&expected).unwrap(),
                    actual_bytes.len(),
                    bytes(&case.after_hex).len()
                ));
            }
        }
    }
    assert_eq!(targets.len(), 2);
    assert_eq!(cops.len(), 2);
    assert_eq!(headers.len(), 3);
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.into_iter().take(30).collect::<Vec<_>>().join("\n")
    );
}
