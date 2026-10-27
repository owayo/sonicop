//! AutoGenConfigFormatter の console と、別経路の設定生成を実 CLI の記録で検査する。
mod support;

use std::fs;
use std::path::Path;
use support::project::{command, project_without_pinned_ruby};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/autogenconf_console.json")).unwrap()
}

fn decode_hex(hex: &str) -> Vec<u8> {
    let (pairs, remainder) = hex.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn project(case: &serde_json::Value) -> tempfile::TempDir {
    let directory = project_without_pinned_ruby(&[]);
    if let Some(directories) = case["directories"].as_array() {
        for path in directories {
            fs::create_dir_all(directory.path().join(path.as_str().unwrap())).unwrap();
        }
    }
    for (path, contents) in case["files"].as_object().unwrap() {
        fs::write(directory.path().join(path), contents.as_str().unwrap()).unwrap();
    }
    if let Some(seconds) = case["mtime"].as_u64() {
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        for path in case["files"].as_object().unwrap().keys() {
            fs::OpenOptions::new()
                .write(true)
                .open(directory.path().join(path))
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
    }
    directory
}

fn invoke(case: &serde_json::Value, cwd: &Path) -> std::process::Output {
    let args: Vec<_> = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let mut invocation = command(cwd);
    invocation
        .env("PATH", "")
        .env_remove("RUBOCOP_OPTS")
        .env_remove("NO_COLOR")
        .args(args);
    if let Some(source) = case["input_hex"].as_str() {
        invocation.write_stdin(decode_hex(source));
    }
    invocation
        .assert()
        .code(case["exit"].as_i64().unwrap() as i32)
        .get_output()
        .clone()
}

fn report_text(bytes: &[u8]) -> String {
    // #102 の transport と分離し、ANSI 色・空行を含む renderer 本文は変更せず照合する。
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .replace("\r\n", "\n")
}

#[test]
fn autogenconf_console_matches_recorded_upstream() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 33);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let directory = project(case);
        let output = invoke(case, directory.path());
        let mut stdout = output.stdout.as_slice();
        if let Some(expected) = case["corrected_stdin_hex"].as_str() {
            let separator = b"====================";
            let at = stdout
                .windows(separator.len())
                .position(|bytes| bytes == separator)
                .unwrap();
            let tail = &stdout[at + separator.len()..];
            let tail = tail
                .strip_prefix(b"\r\n")
                .or_else(|| tail.strip_prefix(b"\n"))
                .unwrap();
            assert_eq!(tail, decode_hex(expected), "{name}: corrected stdin");
            stdout = &stdout[..at];
        }
        assert_eq!(report_text(stdout), case["stdout"], "{name}: stdout");
        assert_eq!(
            report_text(&output.stderr),
            case["stderr"],
            "{name}: stderr"
        );
        for (path, expected) in case["files_after"].as_object().unwrap() {
            assert_eq!(
                fs::read(directory.path().join(path)).unwrap(),
                decode_hex(expected.as_str().unwrap()),
                "{name}: {path}"
            );
        }
        for (path, expected) in case["outputs_after"].as_object().unwrap() {
            assert_eq!(
                report_text(&fs::read(directory.path().join(path)).unwrap()),
                report_text(&decode_hex(expected.as_str().unwrap())),
                "{name}: {path}"
            );
        }
        assert!(case["generated_files"].as_object().unwrap().is_empty());
        assert!(
            !directory.path().join(".rubocop_todo.yml").exists(),
            "{name}"
        );
        assert!(!directory.path().join(".rubocop.yml").exists(), "{name}");
    }
}

#[test]
fn console_formatter_does_not_change_todo_generation() {
    let fixture = fixture();
    for case in fixture["todo_controls"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let directory = project(case);
        invoke(case, directory.path());
        // header の製品版表示は別経路の既存差。生成された YAML の設定値を本家と照合する。
        for (path, expected) in [
            (
                ".rubocop_todo.yml",
                &case["generated_files"][".rubocop_todo.yml"],
            ),
            (".rubocop.yml", &case["files_after"][".rubocop.yml"]),
        ] {
            let actual: serde_yaml_ng::Value =
                serde_yaml_ng::from_slice(&fs::read(directory.path().join(path)).unwrap()).unwrap();
            let expected: serde_yaml_ng::Value =
                serde_yaml_ng::from_slice(&decode_hex(expected.as_str().unwrap())).unwrap();
            assert_eq!(actual, expected, "{name}: {path}");
        }
        for (path, expected) in case["files_after"].as_object().unwrap() {
            if path.ends_with(".rb") {
                assert_eq!(
                    fs::read(directory.path().join(path)).unwrap(),
                    decode_hex(expected.as_str().unwrap()),
                    "{name}: {path}"
                );
            }
        }
    }
}
