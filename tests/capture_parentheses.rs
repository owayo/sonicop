//! 捕捉 Symbol の符号化差による対象括弧補正と、無害な補正のディスク境界。

mod support;

use std::fs;
use support::project::{command, project_without_pinned_ruby, recorded_disk_bytes};

fn bytes(value: &serde_json::Value) -> Vec<u8> {
    value
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/capture_parentheses_reports.json")).unwrap()
}

fn assert_one_target(report: &serde_json::Value) {
    assert_eq!(report["summary"]["inspected_file_count"], 1);
    assert_eq!(report["summary"]["target_file_count"], 1);
    assert_eq!(report["files"].as_array().unwrap().len(), 1);
    assert_eq!(report["files"][0]["path"], "example.rb");
}

#[test]
fn capture_parentheses_preserve_first_original_bytes_and_uncorrected_diagnostics() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 36);
    for case in cases {
        let source = bytes(&case["source_hex"]);
        let directory = project_without_pinned_ruby(&[(
            ".rubocop.yml",
            "AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n",
        )]);
        let path = directory.path().join("example.rb");
        fs::write(&path, &source).unwrap();
        let mut run = command(directory.path());
        run.env("PATH", "").args([
            "--cache",
            "false",
            "--only",
            case["only"].as_str().unwrap(),
            "-f",
            "json",
        ]);
        let mode = case["mode"].as_str().unwrap();
        if mode != "inspect" {
            run.arg(if mode == "safe" { "-a" } else { "-A" });
        }
        let output = run
            .arg("example.rb")
            .assert()
            .code(if mode == "inspect" { 1 } else { 2 })
            .get_output()
            .clone();
        let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_one_target(&actual);
        let original = cases
            .iter()
            .find(|original| {
                original["mode"] == "inspect"
                    && original["case"] == case["case"]
                    && original["encoding"] == case["encoding"]
                    && original["ending"] == case["ending"]
            })
            .unwrap();
        assert_eq!(
            actual["files"][0]["offenses"], original["upstream_offenses"],
            "{case}"
        );
        assert_eq!(fs::read(&path).unwrap(), source);
        if mode != "inspect" {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(
                    "parentheses would change a regexp capture variable into a method call"
                )
            );
            // stdin もディスクへ書く予定の byte を検査し、拒否時は最初の原本を返す。
            let output = command(directory.path())
                .env("PATH", "")
                .args([
                    "--cache",
                    "false",
                    "--only",
                    case["only"].as_str().unwrap(),
                    if mode == "safe" { "-a" } else { "-A" },
                    "--stdin",
                    "example.rb",
                    "-f",
                    "simple",
                ])
                .write_stdin(source.clone())
                .assert()
                .code(2)
                .get_output()
                .clone();
            assert!(output.stdout.ends_with(&source));
            assert_eq!(fs::read(&path).unwrap(), source);
        }
    }
}

#[test]
fn harmless_capture_encoding_corrections_keep_the_reference_edits() {
    let fixture = fixture();
    let cases = fixture["controls"].as_array().unwrap();
    assert_eq!(cases.len(), 90);
    for case in cases {
        let source = bytes(&case["source_hex"]);
        let directory = project_without_pinned_ruby(&[(
            ".rubocop.yml",
            "AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n",
        )]);
        let path = directory.path().join("example.rb");
        fs::write(&path, &source).unwrap();
        let mut run = command(directory.path());
        run.env("PATH", "").args([
            "--cache",
            "false",
            "--only",
            case["only"].as_str().unwrap(),
            "-f",
            "json",
        ]);
        let mode = case["mode"].as_str().unwrap();
        if mode != "inspect" {
            run.arg(if mode == "safe" { "-a" } else { "-A" });
        }
        let output = run.arg("example.rb").output().unwrap();
        assert!(!String::from_utf8_lossy(&output.stderr).contains("parentheses would change"));
        let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_one_target(&actual);
        let expected: Vec<_> = case["upstream_offenses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|offense| offense["cop_name"] != "Lint/Syntax")
            .cloned()
            .collect();
        // group の本家終了値を個別ファイルの値へ流用せず、残る未補正診断から決める。
        let expected_exit = i32::from(expected.iter().any(|offense| offense["corrected"] != true));
        let rewritten =
            mode != "inspect" && expected.iter().any(|offense| offense["corrected"] == true);
        assert_eq!(output.status.code(), Some(expected_exit), "{case}");
        assert_eq!(
            actual["files"][0]["offenses"],
            serde_json::Value::from(expected),
            "{case}"
        );
        // 本家が失う元符号化・CRLF だけを復元した実出力。診断の期待値は本家の記録から取る。
        assert_eq!(
            fs::read(&path).unwrap(),
            recorded_disk_bytes(&source, &bytes(&case["expected_safe_hex"]), rewritten, 0),
            "{case}"
        );
    }
}

#[test]
fn unicode_capture_symbols_do_not_turn_legacy_method_calls_into_local_reads() {
    let fixture = fixture();
    let cases = fixture["reverse"].as_array().unwrap();
    assert_eq!(cases.len(), 48);
    for case in cases {
        let source = bytes(&case["source_hex"]);
        let directory = project_without_pinned_ruby(&[(
            ".rubocop.yml",
            "AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n",
        )]);
        let path = directory.path().join("example.rb");
        fs::write(&path, &source).unwrap();
        let mode = case["mode"].as_str().unwrap();
        let mut run = command(directory.path());
        run.env("PATH", "").args([
            "--cache",
            "false",
            "--only",
            case["only"].as_str().unwrap(),
            "-f",
            "json",
        ]);
        if mode != "inspect" {
            run.arg(if mode == "safe" { "-a" } else { "-A" });
        }
        let exit = i32::from(case["only_label"] == "combined" && mode == "inspect");
        let output = run
            .arg("example.rb")
            .assert()
            .code(exit)
            .get_output()
            .clone();
        let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_one_target(&actual);
        // 実 MRI が呼ぶメソッドと同じ本家対照。capture を ASCII 名へ変えただけで位置は保つ。
        assert_eq!(
            actual["files"][0]["offenses"], case["method_control_offenses"],
            "{case}"
        );
        let rewritten = mode != "inspect"
            && case["method_control_offenses"]
                .as_array()
                .unwrap()
                .iter()
                .any(|offense| offense["corrected"] == true);
        assert_eq!(
            fs::read(&path).unwrap(),
            recorded_disk_bytes(&source, &bytes(&case["expected_safe_hex"]), rewritten, 0),
            "{case}"
        );
    }
}

#[test]
fn current_capture_guards_preserve_mixed_and_original_bare_cr_bytes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/capture_line_endings.json")).unwrap();
    let safe_fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/capture_line_endings_safe.json")).unwrap();
    let safe_cases = safe_fixture["records"].as_array().unwrap();
    assert_eq!(safe_cases.len(), 6);
    let cases = fixture["records"].as_array().unwrap();
    assert_eq!(cases.len(), 18);
    let mut corrections = 0;
    let mut inspections = 0;
    let mut safe_openings = 0;
    let mut safe_reruns = 0;
    for case in cases {
        let directory =
            project_without_pinned_ruby(&[(".rubocop.yml", case["config"].as_str().unwrap())]);
        let path = directory.path().join("example.rb");
        let source = bytes(&case["source_hex"]);
        fs::write(&path, &source).unwrap();
        let mode = case["mode"].as_str().unwrap();
        let guarding = case["danger"] == true && mode != "inspect";
        let original = cases
            .iter()
            .find(|original| original["id"] == case["id"] && original["mode"] == "inspect")
            .unwrap();
        // 本家はUTF-8を書いて元codecの再読込で止まる。無害対照だけは元codecへ戻した
        // 実CLIの連鎖を使い、途中fatalや未到達の括弧を完成結果へ流用しない。
        let safe_case = if !guarding && mode != "inspect" {
            Some(
                safe_cases
                    .iter()
                    .find(|safe| safe["id"] == case["id"] && safe["mode"] == case["mode"])
                    .unwrap(),
            )
        } else {
            None
        };
        let (offenses, expected_bytes, exit) = if guarding {
            (&original["upstream_offenses"], source.clone(), 2)
        } else if let Some(safe) = safe_case {
            assert_eq!(safe["source_hex"], case["source_hex"]);
            let expected = bytes(&safe["expected_safe_hex"]);
            assert_eq!(
                expected.iter().filter(|byte| **byte == b'(').count(),
                source.iter().filter(|byte| **byte == b'(').count() + 1
            );
            safe_openings += 1;
            (
                &safe["expected_offenses"],
                expected,
                safe["expected_exit"].as_i64().unwrap() as i32,
            )
        } else {
            (
                &case["upstream_offenses"],
                source.clone(),
                case["upstream_exit"].as_i64().unwrap() as i32,
            )
        };
        let mut run = command(directory.path());
        run.env("PATH", "").args([
            "--cache",
            "false",
            "--only",
            case["only"].as_str().unwrap(),
            "-f",
            "json",
        ]);
        if mode != "inspect" {
            run.arg(mode);
        }
        let output = run.arg("example.rb").output().unwrap();
        assert_eq!(output.status.code(), Some(exit), "{case}");
        let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_one_target(&actual);
        assert_eq!(
            actual["summary"]["offense_count"],
            offenses.as_array().unwrap().len()
        );
        assert_eq!(&actual["files"][0]["offenses"], offenses, "{case}");
        assert_eq!(fs::read(&path).unwrap(), expected_bytes, "{case}");
        let reason = "parentheses would change a regexp capture variable into a method call";
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).contains(reason),
            guarding
        );
        if mode == "inspect" {
            inspections += 1;
        } else {
            corrections += 1;
            // stdinも同じraw sourceを使い、diskの途中結果を次の原本へ流用しない。
            let output = command(directory.path())
                .env("PATH", "")
                .args([
                    "--cache",
                    "false",
                    "--only",
                    case["only"].as_str().unwrap(),
                    mode,
                    "--stdin",
                    "example.rb",
                    "-f",
                    "simple",
                ])
                .write_stdin(source)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(exit), "{case}");
            assert!(output.stdout.ends_with(&expected_bytes), "{case}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr).contains(reason),
                guarding
            );
            assert_eq!(fs::read(&path).unwrap(), expected_bytes);
            corrections += 1;
            if !guarding {
                let output = command(directory.path())
                    .env("PATH", "")
                    .args([
                        "--cache",
                        "false",
                        "--only",
                        case["only"].as_str().unwrap(),
                        mode,
                        "-f",
                        "json",
                        "example.rb",
                    ])
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(0), "{case}");
                assert_eq!(fs::read(&path).unwrap(), expected_bytes);
                assert!(!String::from_utf8_lossy(&output.stderr).contains(reason));
                let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_one_target(&actual);
                assert_eq!(actual["summary"]["offense_count"], 0);
                assert_eq!(actual["files"][0]["offenses"], serde_json::json!([]));
                safe_reruns += 1;
            }
        }
    }
    assert_eq!(inspections, 6);
    assert_eq!(corrections, 24);
    assert_eq!(safe_openings, 6);
    assert_eq!(safe_reruns, 6);
}
