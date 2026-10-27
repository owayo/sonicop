//! 本家の単一 formatter 実出力で ANSI の開始・reset位置を含めて比較する。
use std::{fs, process::Command};

use serde_json::Value;

fn bytes(hex: &Value) -> Vec<u8> {
    let hex = hex.as_str().unwrap().as_bytes();
    assert!(hex.len().is_multiple_of(2));
    hex.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn native_bytes(hex: &Value) -> Vec<u8> {
    let raw = bytes(hex);
    if cfg!(windows) {
        raw
    } else {
        // Windows の実 byte は fixture に残し、Unix では textmode の CRLF だけを投影する。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn check_case(case: &Value) {
    let name = case["name"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("case.rb");
    let config = root.path().join("config.yml");
    fs::write(&path, bytes(&case["source_hex"])).unwrap();
    fs::write(&config, bytes(&case["configuration_hex"])).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let arguments: Vec<_> = case["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|argument| argument.as_str().unwrap())
        .collect();
    let output = Command::new(env!("CARGO_BIN_EXE_sonicop"))
        .current_dir(root.path())
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .env_remove("RUBOCOP_OPTS")
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(&arguments)
        .output()
        .unwrap();
    let expected = &case["expected"];
    assert_eq!(
        output.status.code(),
        Some(expected["exit"].as_i64().unwrap() as i32),
        "{name}"
    );
    assert_eq!(
        output.stderr,
        native_bytes(&expected["stderr_hex"]),
        "{name}"
    );
    assert_eq!(
        output.stdout,
        native_bytes(&expected["stdout_hex"]),
        "{name}"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        native_bytes(&expected["source_after_hex"]),
        "{name}"
    );
    assert_eq!(
        fs::read(config).unwrap(),
        bytes(&case["configuration_hex"]),
        "{name}"
    );
    if !arguments.contains(&"-a") {
        assert_eq!(
            fs::metadata(path).unwrap().modified().unwrap(),
            modified,
            "{name}"
        );
    }
}

#[test]
fn simple_and_clang_colors_match_recorded_rubocop() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_colors.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 22);
    for case in cases {
        check_case(case);
    }
}

#[test]
fn tap_keeps_its_status_and_backtick_color_override() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_colors.json")).unwrap();
    let cases = fixture["tap_controls"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        check_case(case);
    }
}

#[test]
fn partial_correction_and_unicode_annotations_match_recorded_rubocop() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_color_boundaries.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        check_case(case);
    }
}
