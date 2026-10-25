//! Windows の実 CLI 記録。解析 view・raw view・保存する物理 bytes を分けて照合する。
#![cfg(windows)]

use serde_json::Value;
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

fn bytes(value: &Value) -> Vec<u8> {
    let hex = value.as_str().unwrap();
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn reference_fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/windows_corrections.json")).unwrap();
    for (section, count) in [
        ("groups", 8),
        ("tail_groups", 4),
        ("removals", 8),
        ("continuations", 8),
        ("stdin", 3),
    ] {
        assert_eq!(
            fixture[section].as_array().unwrap().len(),
            count,
            "{section}"
        );
    }
    assert_eq!(fixture["sources"].as_object().unwrap().len(), 11);
    assert_eq!(fixture["tail_cases"].as_object().unwrap().len(), 11);
    for section in ["groups", "tail_groups", "removals"] {
        for group in fixture[section].as_array().unwrap() {
            assert_eq!(group["runs"].as_array().unwrap().len(), 2, "{section}");
        }
    }
    assert_eq!(fixture["composite"]["runs"].as_array().unwrap().len(), 2);
    // 意図差の向きと全 bytes を本家記録へ結び、古い例外だけを残すことを防ぐ。
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let entries = manifest["windows_native_tail_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(entries.len(), 3);
    for (index, name) in [(0, "lf.rb"), (1, "nul.rb")] {
        assert_eq!(
            entries[index]["source"].as_str().unwrap().as_bytes(),
            bytes(&fixture["tail_cases"][name]["source"])
        );
        assert_eq!(
            entries[index]["upstream"].as_str().unwrap().as_bytes(),
            bytes(&fixture["tail_groups"][0]["runs"][0]["raw_after"][name])
        );
        assert_eq!(
            entries[index]["sonicop"].as_str().unwrap().as_bytes(),
            bytes(&fixture["tail_cases"][name]["after"])
        );
    }
    for (key, recorded) in [
        ("source", &fixture["composite"]["source"]),
        ("upstream", &fixture["composite"]["reference"]["after"]),
        ("sonicop", &fixture["composite"]["runs"][0]["after"]),
    ] {
        assert_eq!(
            entries[2][key].as_str().unwrap().as_bytes(),
            bytes(recorded)
        );
    }
    fixture
}

fn invocation(folder: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(folder)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(["--cache", "false", "-f", "json"]);
    command
}

fn assert_report(output: &std::process::Output, expected: &Value, label: &str) {
    assert!(
        output.stderr.is_empty(),
        "{label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(expected["exit"].as_i64().unwrap() as i32),
        "{label}"
    );
    let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    actual.as_object_mut().unwrap().remove("metadata");
    assert_eq!(actual, expected["report"], "{label}");
}

#[test]
fn windows_disk_correction_uses_the_reference_write_read_cycle() {
    let fixture = reference_fixture();
    let sources = fixture["sources"].as_object().unwrap();
    for group in fixture["groups"].as_array().unwrap() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(
            folder.path().join(".rubocop.yml"),
            group["config"].as_str().unwrap(),
        )
        .unwrap();
        for (name, source) in sources {
            fs::write(folder.path().join(name), bytes(source)).unwrap();
        }
        for (index, run) in group["runs"].as_array().unwrap().iter().enumerate() {
            let mode = if index == 0 {
                group["mode"].as_str().unwrap()
            } else {
                "-A"
            };
            let output = invocation(folder.path())
                .arg(mode)
                .args(
                    run["report"]["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|file| file["path"].as_str().unwrap()),
                )
                .output()
                .unwrap();
            let label = format!("{}/{mode}/{index}", group["style"]);
            assert_report(&output, run, &label);
            for (name, expected) in run["after"].as_object().unwrap() {
                assert_eq!(
                    fs::read(folder.path().join(name)).unwrap(),
                    bytes(expected),
                    "{label}/{name}"
                );
            }
        }
    }
}

#[test]
fn windows_stdin_correction_does_not_emulate_a_disk_cycle() {
    let fixture = reference_fixture();
    let folder = tempfile::tempdir().unwrap();
    fs::write(
        folder.path().join(".rubocop.yml"),
        fixture["stdin_config"].as_str().unwrap(),
    )
    .unwrap();
    for run in fixture["stdin"].as_array().unwrap() {
        let name = run["name"].as_str().unwrap();
        let mut child = invocation(folder.path())
            .args(["-a", "--stdin", name])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&bytes(&fixture["sources"][name]))
            .unwrap();
        assert_report(&child.wait_with_output().unwrap(), run, name);
    }
}

#[test]
fn windows_raw_source_stops_empty_lines_from_correcting_a_later_pass() {
    let fixture = reference_fixture();
    let folder = tempfile::tempdir().unwrap();
    fs::write(
        folder.path().join(".rubocop.yml"),
        fixture["empty_config"].as_str().unwrap(),
    )
    .unwrap();
    for case in fixture["removals"].as_array().unwrap() {
        fs::write(folder.path().join("input.rb"), bytes(&case["source"])).unwrap();
        let only = format!(
            "{},Layout/EmptyLines,Layout/TrailingWhitespace",
            case["cop"].as_str().unwrap()
        );
        for (index, run) in case["runs"].as_array().unwrap().iter().enumerate() {
            let mode = if index == 0 {
                case["mode"].as_str().unwrap()
            } else {
                "-A"
            };
            let output = invocation(folder.path())
                .args([mode, "--only", &only, "input.rb"])
                .output()
                .unwrap();
            assert_report(&output, run, &only);
            assert_eq!(
                fs::read(folder.path().join("input.rb")).unwrap(),
                bytes(&run["after"])
            );
        }
    }
}

#[test]
fn windows_raw_source_keeps_the_intentional_crlf_reparse_correction() {
    let folder = tempfile::tempdir().unwrap();
    fs::write(folder.path().join(".rubocop.yml"), "AllCops:\n  DisabledByDefault: true\n  TargetRubyVersion: 2.7\nLayout/EndOfLine:\n  Enabled: false\n").unwrap();
    // manifest の blind_spots に従い、CRLF で同値な括弧除去を本家の取りこぼしに合わせない。
    // 符号化が変わる断片は #85 の保護を続ける。
    for (literal, removes) in [(r#""\x7F""#, true), (r#""\xFF""#, false)] {
        let source =
            format!("# coding: ASCII-8BIT\r\ndef f\r\n  foo(((17)), {literal})\r\nend\r\n");
        fs::write(folder.path().join("input.rb"), &source).unwrap();
        let output = invocation(folder.path())
            .args(["-A", "--only", "Style/RedundantParentheses", "input.rb"])
            .output()
            .unwrap();
        assert!(output.stderr.is_empty());
        assert_eq!(output.status.code(), Some(0));
        let expected = if removes {
            source.replace("((17))", "17")
        } else {
            source
        };
        assert_eq!(
            fs::read(folder.path().join("input.rb")).unwrap(),
            expected.as_bytes()
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["files"][0]["offenses"]
                .as_array()
                .unwrap()
                .is_empty(),
            !removes
        );
    }
}

#[test]
fn windows_disk_correction_preserves_data_nul_and_false_marker_values() {
    let fixture = reference_fixture();
    let cases = fixture["tail_cases"].as_object().unwrap();
    for group in fixture["tail_groups"].as_array().unwrap() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(
            folder.path().join(".rubocop.yml"),
            group["config"].as_str().unwrap(),
        )
        .unwrap();
        for (name, case) in cases {
            assert_eq!(case["before_value"], case["safe_value"], "{name}");
            fs::write(folder.path().join(name), bytes(&case["source"])).unwrap();
        }
        for (index, run) in group["runs"].as_array().unwrap().iter().enumerate() {
            let mode = if index == 0 {
                group["mode"].as_str().unwrap()
            } else {
                "-A"
            };
            let output = invocation(folder.path())
                .arg(mode)
                .args(
                    run["report"]["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|file| file["path"].as_str().unwrap()),
                )
                .output()
                .unwrap();
            assert_report(&output, run, "tail");
            for (name, case) in cases {
                assert_eq!(
                    fs::read(folder.path().join(name)).unwrap(),
                    bytes(&case["after"]),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn windows_raw_source_continuation_checks_keep_raw_crlf_distinct() {
    let fixture = reference_fixture();
    let folder = tempfile::tempdir().unwrap();
    fs::write(
        folder.path().join(".rubocop.yml"),
        fixture["empty_config"].as_str().unwrap(),
    )
    .unwrap();
    for case in fixture["continuations"].as_array().unwrap() {
        fs::write(folder.path().join("input.rb"), bytes(&case["source"])).unwrap();
        let mut command = invocation(folder.path());
        command.args([
            "--only",
            &format!(
                "{},Layout/SpaceAroundOperators",
                case["cop"].as_str().unwrap()
            ),
        ]);
        if case["mode"] != "inspect" {
            command.arg(case["mode"].as_str().unwrap());
        }
        let output = command.arg("input.rb").output().unwrap();
        assert_report(&output, &case["run"], "continuation");
        assert_eq!(
            fs::read(folder.path().join("input.rb")).unwrap(),
            bytes(&case["run"]["after"])
        );
    }
}

#[test]
fn windows_preserved_data_raw_guard_reaches_a_fixed_point_in_the_first_run() {
    let fixture = reference_fixture();
    let case = &fixture["composite"];
    let folder = tempfile::tempdir().unwrap();
    fs::write(
        folder.path().join(".rubocop.yml"),
        case["config"].as_str().unwrap(),
    )
    .unwrap();
    fs::write(folder.path().join("input.rb"), bytes(&case["source"])).unwrap();
    for run in case["runs"].as_array().unwrap() {
        let output = invocation(folder.path())
            .args([
                "-A",
                "--only",
                "Lint/UselessAccessModifier,Layout/EmptyLines,Layout/TrailingWhitespace",
                "input.rb",
            ])
            .output()
            .unwrap();
        assert_report(&output, run, "DATA quick guard");
        assert_eq!(
            fs::read(folder.path().join("input.rb")).unwrap(),
            bytes(&run["after"])
        );
    }
}

#[test]
fn windows_noop_withheld_and_rolled_back_corrections_do_not_write() {
    let folder = tempfile::tempdir().unwrap();
    let config = "AllCops:\n  DisabledByDefault: true\n  TargetRubyVersion: 2.7\nStyle/NumericPredicate:\n  EnforcedStyle: comparison\n";
    fs::write(folder.path().join(".rubocop.yml"), config).unwrap();
    for (cop, source, exit) in [
        ("Layout/EndOfLine", "x = 1\n", 1),
        (
            "Style/RedundantParentheses",
            "# coding: ASCII-8BIT\ndef f\n  foo((17), \"\\xFF\")\nend\n",
            0,
        ),
        ("Style/NumericPredicate", "foo.zero? {}\n", 2),
    ] {
        let path = folder.path().join("input.rb");
        fs::write(&path, source).unwrap();
        let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let output = invocation(folder.path())
            .args(["-A", "--only", cop, "input.rb"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{cop}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&path).unwrap(), source.as_bytes(), "{cop}");
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "{cop}"
        );
    }
}
