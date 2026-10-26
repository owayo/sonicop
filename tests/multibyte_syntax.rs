//! Prism の byte 診断を文字位置へ投影してから、ゼロ幅を広げる順序を実 CLI と照合する。
mod support;

use std::fs;
use support::project::{command, project_with_ruby};

#[test]
fn multibyte_syntax_ranges_match_the_recorded_cli() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/multibyte_syntax_reports.json")).unwrap();
    let sources = fixture["sources"].as_array().unwrap();
    for group in fixture["groups"].as_array().unwrap() {
        let target = group["target"].as_str().unwrap();
        let parser = group["parser"].as_str().unwrap();
        let directory = project_with_ruby(&[], target);
        fs::write(
            directory.path().join(".rubocop.yml"),
            group["yaml"].as_str().unwrap(),
        )
        .unwrap();
        let mut invocation = command(directory.path());
        // 不正な符号化宣言も復号前の入力で照合し、外部 Ruby なしで診断させる。
        invocation.env("PATH", "").args([
            "--cache",
            "false",
            "--only",
            "Lint/Syntax",
            "--format",
            "json",
        ]);
        let original: Vec<_> = sources
            .iter()
            .map(|source| {
                let path = source["path"].as_str().unwrap();
                let bytes: Vec<u8> = source["bytes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|byte| u8::try_from(byte.as_u64().unwrap()).unwrap())
                    .collect();
                fs::write(directory.path().join(path), &bytes).unwrap();
                invocation.arg(path);
                (path, bytes)
            })
            .collect();
        let output = invocation
            .assert()
            .code(group["exit"].as_i64().unwrap() as i32)
            .get_output()
            .clone();
        assert!(
            output.stderr.is_empty(),
            "{target} {parser}: {:?}",
            output.stderr
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let actual = report["files"].as_array().unwrap();
        let expected = group["files"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{target} {parser}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual, expected, "{target} {parser} {}", expected["path"]);
        }
        assert_eq!(report["summary"], group["summary"], "{target} {parser}");
        for (path, bytes) in original {
            assert_eq!(
                fs::read(directory.path().join(path)).unwrap(),
                bytes,
                "{target} {parser} {path}"
            );
        }
    }
}
