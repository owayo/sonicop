//! 期待値は実 RuboCop CLI と MRI 2.7.8 の記録。raw NUL と escape の AST 分類を区別する。
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
    ending: String,
    mode: String,
    kind: String,
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
fn nul_string_literals_follow_the_real_parser_tokens() {
    let groups: Vec<Group> =
        serde_json::from_str(include_str!("fixtures/string_literals_nul.json")).unwrap();
    assert_eq!(groups.len(), 184);
    assert_eq!(
        groups.iter().map(|group| group.files.len()).sum::<usize>(),
        4952
    );
    let mut bytes_checked = 0;
    let mut failures = Vec::new();
    let mut targets = BTreeSet::new();
    for group in groups {
        targets.insert(group.target.clone());
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(".rubocop.yml"), &group.config).unwrap();
        for case in &group.files {
            fs::write(folder.path().join(&case.path), bytes(&case.source_hex)).unwrap();
            // 本家の補正後を同じ実 MRI で実行し、値の bytes と encoding が不変だった条件だけ。
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
            "Style/StringLiterals",
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
            "{}/{}/{}/{}",
            group.target, group.ending, group.mode, group.kind
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
        let files: HashMap<_, _> = report["files"]
            .as_array()
            .unwrap()
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
            bytes_checked += 1;
            if actual != expected
                || fs::read(folder.path().join(&case.path)).unwrap() != bytes(&case.after_hex)
            {
                failures.push(format!(
                    "{label}/{}: offenses {} / {}, bytes {} / {}",
                    case.id,
                    serde_json::to_string(&actual).unwrap(),
                    serde_json::to_string(&expected).unwrap(),
                    fs::read(folder.path().join(&case.path)).unwrap().len(),
                    bytes(&case.after_hex).len()
                ));
            }
        }
    }
    assert_eq!(targets.len(), 7);
    assert_eq!(bytes_checked, 4952);
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.into_iter().take(30).collect::<Vec<_>>().join("\n")
    );
}
