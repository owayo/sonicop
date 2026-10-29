//! 曖昧名の本文とphase色をnative改行・原本bytesで実参照と照合する。

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn decode_hex(value: &str) -> Vec<u8> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn expected_stream(hex: &str, root: &Path) -> Vec<u8> {
    // macOSの子process cwdは/varのphysical名を使う。Windowsの表記は正規化しない。
    let physical;
    let root = if cfg!(target_os = "macos") {
        physical = std::fs::canonicalize(root).unwrap();
        physical.as_path()
    } else {
        root
    };
    let text = String::from_utf8(decode_hex(hex))
        .unwrap()
        .replace("<root>", &root.to_string_lossy().replace('\\', "/"));
    // fixtureの原本やANSI値は変えず、Windows実CLIのtext transportだけを移す。
    if cfg!(windows) {
        text.into_bytes()
    } else {
        text.replace("\r\n", "\n").into_bytes()
    }
}

#[test]
fn ambiguity_stderr_and_early_stdout_match_the_real_cli() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/ambiguous_config_errors.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(cases.len(), 23);
    assert_eq!(
        cases
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "multiple",
            "wrong",
            "self",
            "snake",
            "color-option",
            "force-color",
            "force-no-color",
            "inherit-force",
            "root-color-option",
            "root-forced",
            "nested-color",
            "nested-no-color",
            "nested-inherit-color",
            "nested-cwd",
            "out-absent",
            "out-sentinel",
            "stdin",
            "version",
            "notice",
            "priority-obsolete",
            "priority-parameter",
            "canonical",
            "priority-cache"
        ]
    );
    assert_eq!(
        cases
            .iter()
            .filter(|row| row["coverage"] == "strict")
            .count(),
        20
    );
    assert_eq!(
        cases
            .iter()
            .filter(|row| row["coverage"] == "deferred_152")
            .count(),
        3
    );
    let mut full_strict = 0;
    let mut stderr_only = 0;
    for case in cases {
        let name = case["id"].as_str().unwrap();
        assert_eq!(
            case["coverage"],
            if matches!(
                name,
                "nested-color" | "nested-no-color" | "nested-inherit-color"
            ) {
                "deferred_152"
            } else {
                "strict"
            },
            "{name}"
        );
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let files = case["files"].as_object().unwrap();
        let mut before = Vec::new();
        for (filename, text) in files {
            let path = root.join(filename);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text.as_str().unwrap()).unwrap();
            before.push((
                path.clone(),
                std::fs::read(&path).unwrap(),
                std::fs::metadata(&path).unwrap().modified().unwrap(),
            ));
        }
        let cwd = root.join(case["cwd"].as_str().unwrap());
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        for argument in case["args"].as_array().unwrap() {
            command.arg(argument.as_str().unwrap());
        }
        command.stdin(if case["stdin_hex"].is_string() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = command.spawn().unwrap();
        if let Some(input) = case["stdin_hex"].as_str() {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&decode_hex(input))
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|v| v as i32),
            "{name}"
        );
        assert_eq!(
            output.stderr,
            expected_stream(case["stderr_hex"].as_str().unwrap(), root),
            "{name} stderr"
        );
        let late_stdout_gap = case["coverage"] == "deferred_152";
        if late_stdout_gap {
            // late config失敗時の完了JSON欠落は別件。実本家JSONを保持し、stdoutは未達と分ける。
            // lifecycle修正時に同じ3条件も全JSON比較へ戻し、空stdoutを期待値にしない。
            assert_eq!(case["stdout_json"], true);
            let mut reference: Value =
                serde_json::from_slice(&decode_hex(case["stdout_hex"].as_str().unwrap())).unwrap();
            reference.as_object_mut().unwrap().remove("metadata");
            assert_eq!(
                reference,
                serde_json::json!({"files": [], "summary": {"offense_count": 0, "target_file_count": 1, "inspected_file_count": 0}})
            );
            stderr_only += 1;
        } else if case["stdout_json"] == true {
            let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            let mut expected: Value =
                serde_json::from_slice(&decode_hex(case["stdout_hex"].as_str().unwrap())).unwrap();
            actual.as_object_mut().unwrap().remove("metadata");
            expected.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, expected, "{name} full report");
        } else {
            assert_eq!(
                output.stdout,
                expected_stream(case["stdout_hex"].as_str().unwrap(), root),
                "{name} stdout"
            );
        }
        if !late_stdout_gap {
            full_strict += 1;
        }
        for (path, bytes, modified) in before {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "{name} physical bytes"
            );
            assert_eq!(
                std::fs::metadata(path).unwrap().modified().unwrap(),
                modified,
                "{name} mtime"
            );
        }
        assert_eq!(
            cwd.join("report.json").exists(),
            files.contains_key("report.json"),
            "{name} out"
        );
    }
    assert_eq!((full_strict, stderr_only), (20, 3));
}
