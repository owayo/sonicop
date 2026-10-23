//! 本家の実 CLI と MRI の DATA.read を記録し、コードの補正と DATA の保存を分けて照合する。
use std::fs;
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    id: String,
    path: String,
    source_hex: String,
    first_after_hex: String,
    rerun_after_hex: String,
    first_raw_reference_after_hex: String,
    rerun_raw_reference_after_hex: String,
    data_before_hex: Option<String>,
    first_expected_data_hex: Option<String>,
    rerun_expected_data_hex: Option<String>,
}

#[derive(Deserialize)]
struct Expected {
    exit: i32,
    report: Value,
}

#[derive(Deserialize)]
struct Group {
    target: String,
    ending: String,
    mode: String,
    config: String,
    files: Vec<Case>,
    first: Expected,
    rerun: Expected,
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
fn semicolon_corrections_keep_data_and_only_change_code_tokens() {
    let groups: Vec<Group> =
        serde_json::from_str(include_str!("fixtures/semicolon_data.json")).unwrap();
    assert_eq!(groups.len(), 12);
    assert_eq!(
        groups.iter().map(|group| group.files.len()).sum::<usize>(),
        216
    );
    for group in groups {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(".rubocop.yml"), &group.config).unwrap();
        for case in &group.files {
            fs::write(folder.path().join(&case.path), bytes(&case.source_hex)).unwrap();
            // 正しい本家 bytes を残し、既存の CRLF 保持方針で戻した期待の DATA.read も実測する。
            assert!(!case.first_raw_reference_after_hex.is_empty());
            assert!(!case.rerun_raw_reference_after_hex.is_empty());
            assert_eq!(
                case.data_before_hex, case.first_expected_data_hex,
                "{}",
                case.id
            );
            assert_eq!(
                case.data_before_hex, case.rerun_expected_data_hex,
                "{}",
                case.id
            );
        }
        for (phase, expected) in [("first", &group.first), ("rerun", &group.rerun)] {
            let mut invocation = Command::new(env!("CARGO_BIN_EXE_sonicop"));
            invocation.current_dir(folder.path()).args([
                "--cache",
                "false",
                "--no-parallel",
                "--only",
                "Style/Semicolon",
                "--format",
                "json",
            ]);
            if group.mode != "inspect" {
                invocation.arg(&group.mode);
            }
            let output = invocation
                .args(group.files.iter().map(|case| &case.path))
                .output()
                .unwrap();
            let label = format!("{}/{}/{}/{phase}", group.target, group.ending, group.mode);
            assert_eq!(output.status.code(), Some(expected.exit), "{label}");
            assert!(output.stderr.is_empty(), "{label}: {:?}", output.stderr);
            let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
            let mut wanted = expected.report.clone();
            actual.as_object_mut().unwrap().remove("metadata");
            wanted.as_object_mut().unwrap().remove("metadata");
            assert_eq!(
                actual["summary"]["target_file_count"],
                group.files.len(),
                "{label}"
            );
            assert_eq!(
                actual["summary"]["inspected_file_count"],
                group.files.len(),
                "{label}"
            );
            assert_eq!(actual, wanted, "{label}");
            for case in &group.files {
                let after = if phase == "first" {
                    &case.first_after_hex
                } else {
                    &case.rerun_after_hex
                };
                assert_eq!(
                    fs::read(folder.path().join(&case.path)).unwrap(),
                    bytes(after),
                    "{label}/{}",
                    case.id
                );
            }
        }
    }
}
