//! 正常JSON、拒否gate、既存単独バグとの同値性を分けて検査する。

mod support;

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::Value;
use support::project::{command, project_without_pinned_ruby};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/repeated_selectors.json")).unwrap()
}

fn run(directory: &Path, case: &Value, args: &Value, source: &str) -> Output {
    let mut run = command(directory);
    for name in [
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBYOPT",
        "RUBYLIB",
    ] {
        run.env_remove(name);
    }
    if let Some(value) = case["env_options"].as_str() {
        run.env("RUBOCOP_OPTS", value);
    }
    run.args(
        args.as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap()),
    );
    run.write_stdin(source);
    run.output().unwrap()
}

fn report(data: &[u8]) -> Value {
    let mut report: Value = serde_json::from_slice(data).unwrap();
    report.as_object_mut().unwrap().remove("metadata");
    report
}

fn project(fixture: &Value, case: &Value) -> tempfile::TempDir {
    let directory =
        project_without_pinned_ruby(&[(".rubocop.yml", fixture["yaml"].as_str().unwrap())]);
    fs::write(
        directory.path().join("input.rb"),
        fixture["source"].as_str().unwrap().as_bytes(),
    )
    .unwrap();
    if let Some(contents) = case["dotfile"].as_str() {
        fs::write(directory.path().join(".rubocop"), contents.as_bytes()).unwrap();
    }
    directory
}

#[test]
fn repeated_selectors_replace_the_set_and_preserve_formatter_output_slots() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "full_json")
    {
        count += 1;
        let directory = project(&fixture, case);
        let source = fixture["source"].as_str().unwrap();
        let output = run(directory.path(), case, &case["reference"]["args"], source);
        assert_eq!(
            output.status.code(),
            case["reference"]["exit"].as_i64().map(|code| code as i32),
            "{}: {:?}",
            case["id"],
            output.stderr
        );
        assert!(
            output.stderr.is_empty(),
            "{}: {:?}",
            case["id"],
            output.stderr
        );
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
        assert_eq!(
            fs::read(directory.path().join("input.rb")).unwrap(),
            source.as_bytes(),
            "{}",
            case["id"]
        );
    }
    assert_eq!(count, 56);
}

#[test]
fn required_values_and_other_option_conflicts_are_still_rejected() {
    let fixture = fixture();
    let mut count = 0;
    let mut late_report_controls = 0;
    for case in fixture["cases"].as_array().unwrap().iter().filter(|case| {
        matches!(
            case["coverage"].as_str(),
            Some("rejection_envelope" | "rejection_only")
        )
    }) {
        count += 1;
        let directory = project(&fixture, case);
        let source = fixture["source"].as_str().unwrap();
        assert_eq!(case["reference"]["exit"], 2);
        let output = run(directory.path(), case, &case["reference"]["args"], source);
        assert_eq!(output.status.code(), Some(2), "{}", case["id"]);
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains(case["error_token"].as_str().unwrap()),
            "{}: {error}",
            case["id"]
        );
        if case["coverage"] == "rejection_envelope" {
            assert_eq!(case["reference"]["stdout_hex"], "");
            assert!(output.stdout.is_empty(), "{}", case["id"]);
        } else {
            // #118 のlate error JSONは未一致。ここで拒否できることを完全互換へ数えない。
            assert_eq!(case["residual_issue"], 118);
            late_report_controls += 1;
        }
        assert_eq!(
            fs::read(directory.path().join("input.rb")).unwrap(),
            source.as_bytes(),
            "{}",
            case["id"]
        );
    }
    assert_eq!(count, 28);
    assert_eq!(late_report_controls, 4);
}

#[test]
fn a_last_empty_selector_is_equivalent_to_its_single_occurrence() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "single_equivalence")
    {
        count += 1;
        assert_eq!(case["residual_issue"], 117);
        assert_eq!(case["reference"]["exit"], case["single_reference"]["exit"]);
        assert_eq!(
            case["reference"]["report"],
            case["single_reference"]["report"]
        );
        assert_eq!(
            case["reference"]["stderr_hex"],
            case["single_reference"]["stderr_hex"]
        );
        let directory = project(&fixture, case);
        let source = fixture["source"].as_str().unwrap();
        let repeated = run(directory.path(), case, &case["reference"]["args"], source);
        let single = run(
            directory.path(),
            case,
            &case["single_reference"]["args"],
            source,
        );
        // #117 の単独empty互換は別工程。本家が証明した「重複でも同じ最後の値」を検査する。
        assert_eq!(
            repeated.status.code(),
            single.status.code(),
            "{}",
            case["id"]
        );
        assert_eq!(repeated.stdout, single.stdout, "{}", case["id"]);
        assert_eq!(repeated.stderr, single.stderr, "{}", case["id"]);
        assert_eq!(
            fs::read(directory.path().join("input.rb")).unwrap(),
            source.as_bytes(),
            "{}",
            case["id"]
        );
    }
    assert_eq!(count, 4);
}
