//! 未知builtin formatterの共通見出しと、失敗時の物理File状態を本家で照合する。
use std::{fs, process::Command};

use serde_json::Value;

fn bytes(hex: &Value) -> Vec<u8> {
    let text = hex.as_str().unwrap().as_bytes();
    assert!(text.len().is_multiple_of(2));
    text.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn check_case(fixture: &Value, case: &Value) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("a.rb");
    let config = directory.path().join("config.yml");
    fs::write(&source, bytes(&fixture["source_hex"])).unwrap();
    fs::write(&config, bytes(&fixture["config_hex"])).unwrap();
    for stem in ["first", "second"] {
        fs::write(
            directory.path().join(format!("{stem}.txt")),
            format!("{stem}-original\n"),
        )
        .unwrap();
    }
    let originals: Vec<_> = [&source, &config]
        .iter()
        .map(|path| {
            (
                fs::read(path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory.path());
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(key);
    }
    for argument in case["args"].as_array().unwrap() {
        command.arg(argument.as_str().unwrap());
    }
    let output = command.output().unwrap();
    let name = case["name"].as_str().unwrap();
    assert_eq!(
        output.status.code(),
        Some(case["exit"].as_i64().unwrap() as i32),
        "{name}"
    );
    assert_eq!(output.stdout, bytes(&case["stdout_hex"]), "{name}");
    let expected = bytes(&case["stderr_message_prefix_hex"]);
    let expected = if cfg!(windows) {
        expected
    } else {
        // Windows参照のtextmode改行だけを投影し、名前の原文を保つ。
        String::from_utf8(expected)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    };
    assert!(
        output.stderr.starts_with(&expected),
        "{name}: {:?}",
        output.stderr
    );
    for (path, (body, modified)) in [&source, &config].iter().zip(originals) {
        assert_eq!(fs::read(path).unwrap(), body, "{name}");
        assert_eq!(
            fs::metadata(path).unwrap().modified().unwrap(),
            modified,
            "{name}"
        );
    }
    for (file, expected) in case["files_hex"].as_object().unwrap() {
        assert_eq!(
            fs::read(directory.path().join(file)).unwrap(),
            bytes(expected),
            "{name}/{file}"
        );
    }
}

#[test]
fn unknown_formatter_error_keeps_upstream_file_initialization_order() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_lookup.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 17);
    for case in &cases[..3] {
        check_case(&fixture, case);
    }
}

#[test]
fn unknown_formatter_message_keeps_raw_names_suggestions_and_color() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_lookup.json")).unwrap();
    for case in &fixture["cases"].as_array().unwrap()[3..] {
        check_case(&fixture, case);
    }
}
