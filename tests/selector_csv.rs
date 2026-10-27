//! 本家の実 CLI 記録で、CSV の正常選択と別 Issue の拒否表示を分けて検査する。

mod support;

use std::fs;

use serde_json::Value;
use support::project::{command, project_without_pinned_ruby};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/selector_csv.json")).unwrap()
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

fn report(data: &[u8]) -> Value {
    let mut report: Value = serde_json::from_slice(data).unwrap();
    report.as_object_mut().unwrap().remove("metadata");
    report
}

fn check_case(fixture: &Value, case: &Value) -> std::process::Output {
    let directory =
        project_without_pinned_ruby(&[(".rubocop.yml", fixture["yaml"].as_str().unwrap())]);
    if let Some(paths) = case["directories"].as_array() {
        for path in paths {
            fs::create_dir_all(directory.path().join(path.as_str().unwrap())).unwrap();
        }
    }
    for (path, source) in case["inputs"].as_object().unwrap() {
        fs::create_dir_all(directory.path().join(path).parent().unwrap()).unwrap();
        fs::write(
            directory.path().join(path),
            source.as_str().unwrap().as_bytes(),
        )
        .unwrap();
    }
    let mut run = command(directory.path());
    for name in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        run.env_remove(name);
    }
    run.args(
        case["reference"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap()),
    );
    run.write_stdin(case["stdin_source"].as_str().unwrap());
    let output = run.output().unwrap();
    assert_eq!(
        output.status.code(),
        case["reference"]["exit"].as_i64().map(|code| code as i32),
        "{}: {:?}",
        case["id"],
        output.stderr
    );
    for (path, source) in case["inputs"].as_object().unwrap() {
        assert_eq!(
            fs::read(directory.path().join(path)).unwrap(),
            source.as_str().unwrap().as_bytes(),
            "{} / {path}",
            case["id"]
        );
    }
    if case["coverage"] == "full_json" || case["coverage"] == "rejection_only" {
        if case["reference"]["report"].is_object() {
            assert_eq!(
                report(&output.stdout),
                case["reference"]["report"],
                "{}",
                case["id"]
            );
        } else {
            assert!(output.stdout.is_empty(), "{}", case["id"]);
        }
        let reference_stderr = bytes(case["reference"]["stderr_hex"].as_str().unwrap());
        // fixture は Windows の warn 出力そのもの。POSIX の改行だけを実行 OS に合わせる。
        let expected_stderr = if cfg!(windows) {
            reference_stderr
        } else {
            String::from_utf8(reference_stderr)
                .unwrap()
                .replace("\r\n", "\n")
                .into_bytes()
        };
        assert_eq!(output.stderr, expected_stderr, "{}", case["id"]);
        for (name, expected) in case["outputs"].as_object().unwrap() {
            let path = directory.path().join(name);
            if expected.is_null() {
                assert!(!path.exists(), "{} / {name}", case["id"]);
            } else {
                assert_eq!(
                    report(&fs::read(path).unwrap()),
                    expected["report"],
                    "{} / {name}",
                    case["id"]
                );
            }
        }
    }
    output
}

#[test]
fn empty_lists_and_literal_names_match_upstream_selection_and_warnings() {
    let fixture = fixture();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 114);
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "full_json")
    {
        count += 1;
        check_case(&fixture, case);
    }
    assert_eq!(count, 51);
}

#[test]
fn explicit_empty_and_internal_empty_names_are_rejected_without_trimming() {
    let fixture = fixture();
    let mut late_errors = 0;
    let mut early_errors = 0;
    let mut parse_boundaries = 0;
    for case in fixture["cases"].as_array().unwrap() {
        match case["coverage"].as_str().unwrap() {
            "rejection_only" => {
                let output = check_case(&fixture, case);
                assert_eq!(case["residual_issue"], 118);
                assert!(
                    String::from_utf8(output.stderr)
                        .unwrap()
                        .to_lowercase()
                        .contains("unrecognized cop"),
                    "{}",
                    case["id"]
                );
                // #118でlate errorの空JSONと警告も本家記録へ一致させた。
                late_errors += 1;
            }
            "rejection_envelope" => {
                let output = check_case(&fixture, case);
                assert!(output.stdout.is_empty(), "{}", case["id"]);
                assert!(!output.stderr.is_empty(), "{}", case["id"]);
                early_errors += 1;
            }
            "parse_boundary" => {
                // LF 単独 argv の OptionParser 消費は CSV 分割とは別の残差として原記録を残す。
                assert_eq!(case["reference"]["exit"], 2);
                parse_boundaries += 1;
            }
            "full_json" => {}
            coverage => panic!("unrecognized coverage {coverage}"),
        }
    }
    assert_eq!(late_errors, 47);
    assert_eq!(early_errors, 12);
    assert_eq!(parse_boundaries, 4);
}

#[test]
fn badge_normalization_matches_upstream_qualification_and_warning_names() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/selector_badges.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 32);
    let mut complete = 0;
    let mut late_errors = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let output = check_case(&fixture, case);
        match case["coverage"].as_str().unwrap() {
            "full_json" => complete += 1,
            "rejection_only" => {
                assert_eq!(case["residual_issue"], 118);
                assert!(
                    String::from_utf8(output.stderr)
                        .unwrap()
                        .to_lowercase()
                        .contains("unrecognized cop"),
                    "{}",
                    case["id"]
                );
                // 登録済み正規化badgeの元名を返す分岐も、空JSONとstderr全体を比較する。
                late_errors += 1;
            }
            coverage => panic!("unrecognized coverage {coverage}"),
        }
    }
    assert_eq!(complete, 20);
    assert_eq!(late_errors, 12);
}

#[test]
fn directory_stdin_emits_selection_warnings_once_after_target_discovery() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/selector_directory_warnings.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 6);
    assert_eq!(
        cases
            .iter()
            .map(|case| case["id"].as_str().unwrap())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        6
    );
    let mut complete = 0;
    let mut listing = 0;
    for case in cases {
        let output = check_case(&fixture, case);
        match case["coverage"].as_str().unwrap() {
            "full_json" => complete += 1,
            "listing" => {
                // 一覧は検査前の本文。既知の Windows 改行だけを実行 OS へ投影する。
                let native = |hex: &str| {
                    let raw = bytes(hex);
                    if cfg!(windows) {
                        raw
                    } else {
                        String::from_utf8(raw)
                            .unwrap()
                            .replace("\r\n", "\n")
                            .into_bytes()
                    }
                };
                assert_eq!(
                    output.stdout,
                    native(case["reference"]["stdout_hex"].as_str().unwrap())
                );
                assert_eq!(
                    output.stderr,
                    native(case["reference"]["stderr_hex"].as_str().unwrap())
                );
                listing += 1;
            }
            coverage => panic!("unrecognized coverage {coverage}"),
        }
    }
    assert_eq!(complete, 5);
    assert_eq!(listing, 1);
}
