//! parser gem の回復が診断と補正拒否へ反映されることを、本家 CLI の記録で検査する。
mod support;

use std::fs;
use support::project::{command, project_with_ruby, recorded_disk_bytes};

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
            let rewritten = mode != "lint"
                && expected["offenses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|offense| offense["corrected"] == true);
            let written = recorded_disk_bytes(source.as_bytes(), written.as_bytes(), rewritten, 0);
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                written,
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

#[test]
fn recorded_disk_bytes_keep_noop_mixed_and_typed_data_boundaries() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/windows_corrections.json")).unwrap();
    let decode = |hex: &serde_json::Value| {
        hex.as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>()
    };
    let run = &fixture["groups"][0]["runs"][0];
    for (name, protected_tail) in [
        ("changed.rb", 0),
        ("clean.rb", 0),
        ("mixed.rb", 0),
        ("data.rb", 4),
        ("nul.rb", 5),
    ] {
        let original = decode(&fixture["sources"][name]);
        let windows = decode(&run["after"][name]);
        let recorded = if original.windows(2).any(|pair| pair == b"\r\n") {
            windows.clone()
        } else {
            // Windows実記録を旧OSのLF形式に投影した同じ編集。期待は実記録のまま。
            String::from_utf8(windows.clone())
                .unwrap()
                .replace("\r\n", "\n")
                .into_bytes()
        };
        let rewritten = run["report"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == name)
            .unwrap()["offenses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|offense| offense["corrected"] == true);
        assert_eq!(
            recorded_disk_bytes(&original, &recorded, rewritten, protected_tail),
            if cfg!(windows) { windows } else { recorded },
            "{name}"
        );
        // 同一byteに戻る／補正を保留する原本は、correctedの有無だけでは変換しない。
        assert_eq!(recorded_disk_bytes(&original, &original, true, 0), original);
    }
}
