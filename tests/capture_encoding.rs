//! 捕捉名の符号化で宣言が変わる境界を、本家 CLI の全診断・原本 byte と照合する。
mod support;

use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use support::project::{command, project_with_ruby};

fn decode(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn capture_names_keep_the_compiled_and_source_encodings_distinct() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/capture_encoding.json")).unwrap();
    assert_eq!(fixture["rubocop_version"], "1.89.0");
    let cases = fixture["cases"].as_object().unwrap();
    assert!(!cases.is_empty());
    let names: Vec<_> = cases.keys().map(String::as_str).collect();
    let groups = fixture["groups"].as_array().unwrap();
    let settings: BTreeSet<_> = groups
        .iter()
        .map(|group| {
            (
                group["target"].as_str().unwrap(),
                group["engine"].as_str().unwrap_or("default"),
            )
        })
        .collect();
    let required_settings: BTreeSet<_> = [
        ("2.7", "default"),
        ("2.7", "parser_whitequark"),
        ("3.0", "parser_whitequark"),
        ("3.1", "parser_whitequark"),
        ("3.2", "parser_whitequark"),
        ("3.3", "parser_whitequark"),
        ("3.4", "parser_whitequark"),
        ("3.3", "default"),
        ("3.3", "parser_prism"),
        ("3.4", "parser_prism"),
    ]
    .into_iter()
    .collect();
    assert_eq!(groups.len(), required_settings.len(), "incomplete settings");
    assert_eq!(settings, required_settings, "wrong settings");
    let required_paths: BTreeSet<_> = names.iter().copied().collect();
    for group in groups {
        let target = group["target"].as_str().unwrap();
        let engine = group["engine"].as_str().unwrap_or("default");
        let directory = project_with_ruby(&[], target);
        fs::write(
            directory.path().join(".rubocop.yml"),
            group["config"].as_str().unwrap(),
        )
        .unwrap();
        let runs = group["runs"].as_array().unwrap();
        let modes: BTreeSet<_> = runs
            .iter()
            .map(|run| run["mode"].as_str().unwrap_or("inspect"))
            .collect();
        assert_eq!(runs.len(), 3, "incomplete modes");
        assert_eq!(
            modes,
            BTreeSet::from(["inspect", "-a", "-A"]),
            "wrong modes"
        );
        for run in runs {
            let expected_files = run["files"].as_array().unwrap();
            let expected_paths: BTreeSet<_> = expected_files
                .iter()
                .map(|file| file["path"].as_str().unwrap())
                .collect();
            assert_eq!(
                expected_files.len(),
                cases.len(),
                "incomplete expected files"
            );
            assert_eq!(expected_paths, required_paths, "wrong expected paths");
            let mode = run["mode"].as_str().unwrap_or("inspect");
            for (name, hex) in cases {
                fs::write(directory.path().join(name), decode(hex.as_str().unwrap())).unwrap();
            }
            let mut invocation = command(directory.path());
            // source gem を含む配布物は、Ruby と外部の regexp compiler が無い環境でも完結する。
            invocation.env("PATH", "").args([
                "--cache",
                "false",
                "--only",
                fixture["only"].as_str().unwrap(),
                "--format",
                "json",
            ]);
            if mode != "inspect" {
                invocation.arg(mode);
            }
            let output = invocation.args(&names).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(run["exit"].as_i64().unwrap() as i32),
                "{target} {engine} {mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                report["summary"], run["summary"],
                "{target} {engine} {mode}"
            );
            assert_eq!(
                report["summary"]["inspected_file_count"],
                cases.len(),
                "incomplete run"
            );
            let actual_files = report["files"].as_array().unwrap();
            assert_eq!(actual_files.len(), cases.len(), "duplicate inspected files");
            let actual: BTreeMap<_, _> = actual_files
                .iter()
                .map(|file| {
                    let name = Path::new(file["path"].as_str().unwrap())
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap();
                    (name, &file["offenses"])
                })
                .collect();
            assert_eq!(actual.len(), cases.len());
            assert_eq!(
                actual.keys().copied().collect::<BTreeSet<_>>(),
                required_paths,
                "wrong inspected paths"
            );
            for expected in run["files"].as_array().unwrap() {
                let name = expected["path"].as_str().unwrap();
                assert_eq!(
                    actual.get(name),
                    Some(&&expected["offenses"]),
                    "{target} {engine} {mode} {name}"
                );
                let original = decode(cases[name].as_str().unwrap());
                let mut written = decode(run["written"][name].as_str().unwrap());
                if original.windows(2).any(|pair| pair == b"\r\n") {
                    // 本家記録は保持し、既登録の CRLF 保存差だけを書き戻し byte の照合で戻す。
                    let mut preserved = Vec::with_capacity(written.len());
                    for (offset, &byte) in written.iter().enumerate() {
                        if byte == b'\n' && (offset == 0 || written[offset - 1] != b'\r') {
                            preserved.push(b'\r');
                        }
                        preserved.push(byte);
                    }
                    written = preserved;
                }
                assert_eq!(
                    fs::read(directory.path().join(name)).unwrap(),
                    written,
                    "{target} {engine} {mode} {name} written bytes"
                );
            }
        }
    }
}
