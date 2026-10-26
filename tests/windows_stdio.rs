//! Windows Ruby の text mode と、stdin の最終出力バイトを照合する。
#![cfg(windows)]

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::Value;

fn fixture() -> Value {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/windows_stdio.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 21);
    assert_eq!(fixture["safety_cases"].as_array().unwrap().len(), 9);
    assert_eq!(
        fixture["literal_boundary_cases"].as_array().unwrap().len(),
        14
    );
    assert_eq!(fixture["malformed_controls"].as_array().unwrap().len(), 4);
    fixture
}

fn bytes(value: &Value) -> Vec<u8> {
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

fn run(directory: &Path, source: &[u8], destination: &str, mode: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(directory)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args([
            "--config",
            "config.yml",
            "--cache",
            "false",
            "--only",
            "Layout/SpaceAroundOperators",
            mode,
            "--stdin",
            "input.rb",
        ]);
    match destination {
        "simple-stdout" => command.args(["-f", "simple"]),
        "simple-stderr" => command.args(["-f", "simple", "--stderr"]),
        "simple-file" => command.args(["-f", "simple", "--out", "report.txt"]),
        "json-stdout" => command.args(["-f", "json"]),
        "json-stderr" => command.args(["-f", "json", "--stderr"]),
        "json-file" => command.args(["-f", "json", "--out", "report.txt"]),
        "junit-file" => command.args(["-f", "junit", "--out", "report.txt"]),
        _ => panic!("未知の記録先: {destination}"),
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    child.wait_with_output().unwrap()
}

fn assert_stream(actual: &[u8], expected: &Value, json: bool, label: &str) {
    let expected = bytes(expected);
    if json && !expected.is_empty() {
        // 実装自身の metadata だけを除外し、生改行と診断・配列順は照合する。
        assert_eq!(
            actual.iter().filter(|byte| **byte == b'\n').count(),
            expected.iter().filter(|byte| **byte == b'\n').count(),
            "{label}"
        );
        let mut actual: Value = serde_json::from_slice(actual).unwrap();
        let mut expected: Value = serde_json::from_slice(&expected).unwrap();
        actual.as_object_mut().unwrap().remove("metadata");
        expected.as_object_mut().unwrap().remove("metadata");
        assert_eq!(actual, expected, "{label}");
    } else {
        let offset = actual
            .iter()
            .zip(&expected)
            .position(|(actual, expected)| actual != expected)
            .unwrap_or(actual.len().min(expected.len()));
        assert!(
            actual == expected,
            "{label}: offset {offset}, lengths {} / {}, bytes {:?} / {:?}",
            actual.len(),
            expected.len(),
            &actual[offset..(offset + 20).min(actual.len())],
            &expected[offset..(offset + 20).min(expected.len())]
        );
    }
}

#[test]
fn formatter_streams_files_and_ordinary_stdin_match_windows_textmode() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let destination = case["destination"].as_str().unwrap();
        let label = case["name"].as_str().unwrap();
        let actual = run(
            directory.path(),
            &bytes(&case["source_hex"]),
            destination,
            "-a",
        );
        let expected = &case["reference"];
        assert_eq!(
            actual.status.code(),
            expected["exit"].as_i64().map(|x| x as i32),
            "{label}"
        );
        let json = destination.starts_with("json-");
        assert_stream(&actual.stdout, &expected["stdout_hex"], json, label);
        assert_stream(&actual.stderr, &expected["stderr_hex"], json, label);
        let path = directory.path().join("report.txt");
        if expected["file_hex"].is_null() {
            assert!(!path.exists(), "{label}");
        } else {
            let mut output = fs::read(path).unwrap();
            if destination == "junit-file" {
                let root = directory.path().display().to_string().replace('\\', "/");
                output = String::from_utf8(output)
                    .unwrap()
                    .replace(&root, "${PROJECT}")
                    .into_bytes();
            }
            assert_stream(&output, &expected["file_hex"], json, label);
        }
        assert!(!directory.path().join("input.rb").exists(), "{label}");
    }
}

#[test]
fn stdin_textmode_protects_recorded_values_and_rejects_broken_scripts() {
    let fixture = fixture();
    for case in fixture["safety_cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let label = case["name"].as_str().unwrap();
        let actual = run(
            directory.path(),
            &bytes(&case["source_hex"]),
            "simple-stderr",
            "-a",
        );
        assert_eq!(
            actual.status.code(),
            case["safe"]["exit"].as_i64().map(|x| x as i32),
            "{label}"
        );
        assert_stream(&actual.stdout, &case["safe"]["stdout_hex"], false, label);
        if case["kind"] == "syntax-rejected" {
            assert!(
                String::from_utf8_lossy(&actual.stderr)
                    .contains("because it introduced a syntax error"),
                "{label}"
            );
            assert!(
                actual
                    .stderr
                    .ends_with(&bytes(&case["reference"]["stderr_hex"])),
                "{label}"
            );
        } else {
            assert_stream(
                &actual.stderr,
                &case["reference"]["stderr_hex"],
                false,
                label,
            );
        }
        if !case["original_value"].is_null() {
            assert_eq!(case["original_value"], case["safe_value"], "{label}");
            assert_ne!(case["original_value"], case["upstream_value"], "{label}");
        }
    }
}

#[test]
fn an_integration_formatter_does_not_validate_an_unemitted_stdin_echo() {
    let fixture = fixture();
    for case in fixture["safety_cases"].as_array().unwrap() {
        if case["kind"] != "syntax-rejected" {
            continue;
        }
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let actual = run(
            directory.path(),
            &bytes(&case["source_hex"]),
            "json-stdout",
            "-a",
        );
        assert!(actual.status.success());
        assert!(actual.stderr.is_empty());
        let report: Value = serde_json::from_slice(&actual.stdout).unwrap();
        assert_eq!(report["files"][0]["offenses"], serde_json::json!([]));
        assert_eq!(report["summary"]["inspected_file_count"], 1);
    }
}

#[test]
fn escaped_literal_crlf_keeps_each_independently_recorded_value() {
    let fixture = fixture();
    let mut differences = Vec::new();
    for case in fixture["literal_boundary_cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let label = case["name"].as_str().unwrap();
        let actual = run(
            directory.path(),
            &bytes(&case["source_hex"]),
            "simple-stderr",
            "-a",
        );
        if actual.status.code() != Some(0)
            || actual.stdout != bytes(&case["safe"]["stdout_hex"])
            || actual.stderr != bytes(&case["reference"]["stderr_hex"])
        {
            differences.push(label);
        }
        assert_eq!(case["original_value"], case["safe_value"], "{label}");
        if case["kind"] == "rewrite-control" {
            assert_eq!(case["original_value"], case["upstream_value"], "{label}");
        } else {
            assert_ne!(case["original_value"], case["upstream_value"], "{label}");
        }
    }
    assert!(
        differences.is_empty(),
        "literal 境界の差異: {differences:?}"
    );
}

#[test]
fn malformed_stdin_keeps_the_normal_syntax_exit_and_native_echo() {
    let fixture = fixture();
    for case in fixture["malformed_controls"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let label = case["name"].as_str().unwrap();
        let actual = run(
            directory.path(),
            &bytes(&case["source_hex"]),
            "simple-stderr",
            "-a",
        );
        assert_eq!(actual.status.code(), Some(1), "{label}");
        assert_stream(
            &actual.stdout,
            &case["reference"]["stdout_hex"],
            false,
            label,
        );
        let stderr = String::from_utf8_lossy(&actual.stderr);
        assert!(stderr.contains("Lint/Syntax"), "{label}: {stderr}");
        assert!(!stderr.contains("refusing to print"), "{label}: {stderr}");
    }
}

#[test]
fn stdin_textmode_manifest_keeps_the_reviewed_modes_and_exact_bytes() {
    let fixture = fixture();
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let manifest = serde_json::to_value(manifest).unwrap();
    let manifest = &manifest["windows_stdin_textmode_divergences"];
    assert_eq!(manifest["issue"], 102);
    assert_eq!(manifest["platform"], "windows");
    assert_eq!(
        manifest["reference_fixture"],
        "tests/fixtures/windows_stdio.json"
    );
    for key in ["reference", "config", "only", "transport_token_analysis"] {
        assert_eq!(manifest[key], fixture[key], "{key}");
    }
    assert_eq!(manifest["modes"], fixture["verified_modes"]);
    assert_eq!(manifest["modes"], serde_json::json!(["-a", "-A"]));
    assert_eq!(
        manifest["review"]["implementation_patch_sha256"],
        "efe864da40a043d53b465e1031f4d8b8419196386c74dd09671a2fc9f84a181b"
    );
    let entries = manifest["cases"].as_array().unwrap();
    let proofs = fixture["mode_equivalence"].as_array().unwrap();
    assert_eq!(entries.len(), 16);
    assert_eq!(proofs.len(), 16);
    let mut seen = std::collections::BTreeSet::new();
    for case in fixture["safety_cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(fixture["literal_boundary_cases"].as_array().unwrap())
        .filter(|case| case["kind"] != "rewrite-control")
    {
        let name = case["name"].as_str().unwrap();
        assert!(seen.insert(name), "{name}");
        let entry = entries.iter().find(|entry| entry["case"] == name).unwrap();
        let proof = proofs.iter().find(|proof| proof["name"] == name).unwrap();
        assert_eq!(entry["source_hex"], case["source_hex"], "{name}");
        assert_eq!(proof["source_hex"], case["source_hex"], "{name}");
        assert_eq!(proof["mode"], "-A", "{name}");
        assert_eq!(
            entry["kind"],
            case["kind"].as_str().unwrap_or("literal-value"),
            "{name}"
        );
        for (manifest_key, fixture_key, side) in [
            ("upstream", "reference", "reference"),
            ("sonicop", "safe", "candidate"),
        ] {
            assert_eq!(entry[manifest_key], case[fixture_key], "{name}");
            let mut observed = proof["sides"][side].clone();
            assert_eq!(observed["exact_to_measured_a"], true, "{name}/{side}");
            observed
                .as_object_mut()
                .unwrap()
                .remove("exact_to_measured_a");
            assert_eq!(observed, case[fixture_key], "{name}/{side}");
        }
        if !case["original_value"].is_null() {
            for key in ["original_value", "upstream_value", "safe_value"] {
                assert_eq!(entry["runtime"][key], case[key], "{name}/{key}");
            }
        } else {
            assert!(entry["runtime"].is_null(), "{name}");
        }
        // 独立した両 mode の実 pipe 記録を使う。rewrite 対照を意図差へ混ぜない。
        for mode in fixture["verified_modes"].as_array().unwrap() {
            let directory = tempfile::tempdir().unwrap();
            fs::write(
                directory.path().join("config.yml"),
                fixture["config"].as_str().unwrap(),
            )
            .unwrap();
            let actual = run(
                directory.path(),
                &bytes(&case["source_hex"]),
                "simple-stderr",
                mode.as_str().unwrap(),
            );
            assert_eq!(
                actual.status.code(),
                case["safe"]["exit"].as_i64().map(|exit| exit as i32),
                "{name}/{mode}"
            );
            assert_stream(&actual.stdout, &case["safe"]["stdout_hex"], false, name);
            assert_stream(&actual.stderr, &case["safe"]["stderr_hex"], false, name);
        }
    }
    assert_eq!(seen.len(), entries.len());
}
