//! parser gem の回復が診断と補正拒否へ反映されることを、本家 CLI の記録で検査する。
mod support;

use std::fs;
use support::project::{command, project_with_ruby};

#[test]
fn parser_gem_reports_and_correction_rejection_match_the_recorded_cli() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/legacy_syntax_reports.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let source = case["source"].as_str().unwrap();
        let target = case["target"].as_str().unwrap();
        for expected in case["runs"].as_array().unwrap() {
            let mode = expected["mode"].as_str().unwrap();
            let directory = project_with_ruby(&[("example.rb", source)], target);
            fs::write(
                directory.path().join(".rubocop.yml"),
                case["yaml"].as_str().unwrap(),
            )
            .unwrap();
            let mut invocation = command(directory.path());
            // 配布バイナリは Ruby/parser gem の外部プロセスなしで診断する。
            invocation.env("PATH", "").args([
                "--cache",
                "false",
                "--only",
                case["only"]
                    .as_str()
                    .unwrap_or("Lint/Syntax,Layout/TrailingWhitespace"),
                "--format",
                "json",
            ]);
            if mode != "lint" {
                invocation.arg(mode);
            }
            let output = invocation
                .arg("example.rb")
                .assert()
                .code(expected["exit"].as_i64().unwrap() as i32)
                .get_output()
                .stdout
                .clone();
            let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(report["summary"]["target_file_count"], 1, "{name} {mode}");
            assert_eq!(
                report["summary"]["inspected_file_count"], 1,
                "{name} {mode}"
            );
            assert_eq!(
                report["files"][0]["offenses"], expected["offenses"],
                "{name} {mode}"
            );
            let recorded = expected["written"].as_str().unwrap();
            // 本家の補正は LF に変換するが、登録済みの意図的差分どおり CRLF を保つ。
            let written = if source.contains("\r\n") {
                recorded.replace("\r\n", "\n").replace('\n', "\r\n")
            } else {
                recorded.to_owned()
            };
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                written.as_bytes(),
                "{name} {mode}"
            );
        }
    }
}

#[test]
fn a_deep_legacy_tree_is_destroyed_on_a_worker_sized_stack() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            for source in [
                format!("{}a\n", "a+".repeat(12_000)),
                format!("{}a; def bad(&) = nil\n", "a+".repeat(12_000)),
            ] {
                let (_, diagnostics) =
                    sonicop::legacy_parser::debug_tokens(&source, sonicop::RubyVersion::new(2, 7))
                        .unwrap();
                if source.contains("def bad") {
                    assert!(!diagnostics.is_empty());
                } else {
                    assert!(diagnostics.is_empty());
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
