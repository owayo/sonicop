//! 明示した空 stdin 名の診断・表示・補正 byte を、本家 CLI の記録で照合する。
mod support;

use std::fs;
use std::path::Path;
use support::project::{command, project_without_pinned_ruby};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/empty_stdin_reports.json")).unwrap()
}

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

fn canonical(text: &[u8], cwd: &Path) -> String {
    // macOS の getcwd は一時ディレクトリの /var を物理パスの /private/var として返す。
    #[cfg(target_os = "macos")]
    let cwd = fs::canonicalize(cwd).unwrap();
    let cwd = cwd.to_string_lossy().replace('\\', "/");
    String::from_utf8(text.to_vec())
        .unwrap()
        .replace("\r\n", "\n")
        .replace(&cwd.replace('/', "."), "<CWD_CLASS>")
        .replace(&cwd, "<CWD>")
}

fn project(case: &serde_json::Value) -> tempfile::TempDir {
    let directory = project_without_pinned_ruby(&[]);
    for (name, text) in case["files"].as_object().unwrap() {
        let path = directory.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text.as_str().unwrap()).unwrap();
    }
    directory
}

fn assert_run(
    case: &serde_json::Value,
    expected: &serde_json::Value,
    cwd: &Path,
    source: Vec<u8>,
) -> Option<Vec<u8>> {
    let name = case["name"].as_str().unwrap();
    let args: Vec<_> = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let output = command(cwd)
        .env("PATH", "")
        .args(args)
        .write_stdin(source)
        .assert()
        .code(expected["exit"].as_i64().unwrap() as i32)
        .get_output()
        .clone();
    if case["missing_value"].as_bool() == Some(true) {
        // エラー本文の既存 CLI 表現は保ち、空値の受理と引数省略の拒否を分ける。
        assert!(output.stdout.is_empty(), "{name}");
        assert!(!output.stderr.is_empty(), "{name}");
        return None;
    }
    assert!(output.stderr.is_empty(), "{name}: {:?}", output.stderr);
    let corrected = if let Some(hex) = expected["corrected_hex"].as_str() {
        let marker = b"====================";
        let at = output
            .stdout
            .windows(marker.len())
            .position(|bytes| bytes == marker)
            .unwrap();
        let after = &output.stdout[at + marker.len()..];
        let source = after
            .strip_prefix(b"\r\n")
            .or_else(|| after.strip_prefix(b"\n"))
            .unwrap();
        assert_eq!(source, bytes(hex), "{name}: corrected source");
        assert_eq!(
            canonical(&output.stdout[..at], cwd),
            expected["text"],
            "{name}"
        );
        Some(source.to_vec())
    } else if case["format"] == "json" {
        // #102 が report の終端改行を直した後も、JSON の全フィールドを同じ値で比較する。
        let mut report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        report.as_object_mut().unwrap().remove("metadata");
        assert_eq!(report, expected["report"], "{name}");
        None
    } else {
        assert_eq!(canonical(&output.stdout, cwd), expected["text"], "{name}");
        None
    };
    for (path, text) in case["files"].as_object().unwrap() {
        assert_eq!(
            fs::read(cwd.join(path)).unwrap(),
            text.as_str().unwrap().as_bytes(),
            "{name}: {path}"
        );
    }
    corrected
}

#[test]
fn empty_stdin_json_and_corrections_match_the_recorded_cli() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        if case["missing_value"].as_bool() == Some(true) {
            continue;
        }
        let directory = project(case);
        let mut source = bytes(case["input_hex"].as_str().unwrap());
        if case["name"] == "default-cops-named" && !cfg!(windows) {
            // 本家の EndOfLine の既定値 native に合わせ、全 cop の入力だけをその OS の改行にする。
            assert_eq!(source, b"puts 1+2\r\n");
            source = b"puts 1+2\n".to_vec();
        }
        let corrected = assert_run(case, case, directory.path(), source);
        if let Some(corrected) = corrected {
            let mut rerun_case = case.clone();
            rerun_case["args"] = case["rerun_args"].clone();
            rerun_case["format"] = "json".into();
            assert_run(&rerun_case, &case["rerun"], directory.path(), corrected);
        }
    }
}

#[test]
fn anonymous_and_named_stdin_match_fourteen_recorded_formatters() {
    let fixture = fixture();
    let formats = fixture["formats"].as_array().unwrap();
    assert_eq!(formats.len(), 28);
    for case in formats {
        let directory = project(case);
        assert_run(
            case,
            case,
            directory.path(),
            bytes(case["input_hex"].as_str().unwrap()),
        );
    }
}

#[test]
fn stdin_without_a_value_remains_an_error() {
    let fixture = fixture();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["missing_value"].as_bool() == Some(true))
        .collect();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let directory = project(case);
        assert_run(
            case,
            case,
            directory.path(),
            bytes(case["input_hex"].as_str().unwrap()),
        );
    }
}
