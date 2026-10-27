#![cfg(windows)]

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

/// Rust の argv quoting へ戻さず、実Rubyと同じ生commandlineを子へ渡す。
#[test]
fn native_control_arguments_match_the_reference_cli() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/windows_argv.json")).unwrap();
    let cases = fixture["cli"].as_array().unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let source: Vec<_> = case["source_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let source_path = directory.path().join("input.rb");
        let config_path = directory.path().join("config.yml");
        let config = case["config"].as_str().unwrap();
        std::fs::write(&source_path, &source).unwrap();
        std::fs::write(&config_path, config).unwrap();
        let modified = [
            std::fs::metadata(&source_path).unwrap().modified().unwrap(),
            std::fs::metadata(&config_path).unwrap().modified().unwrap(),
        ];
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(directory.path())
            .raw_arg(case["tail"].as_str().unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        let stdin = case["stdin"].as_bool().unwrap();
        command.stdin(if stdin { Stdio::piped() } else { Stdio::null() });
        let mut child = command.spawn().unwrap();
        if stdin {
            child.stdin.take().unwrap().write_all(&source).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|value| value as i32),
            "{}",
            case["name"]
        );
        assert!(
            output.stderr.is_empty(),
            "{}: {:?}",
            case["name"],
            output.stderr
        );
        let mut report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let metadata = report.as_object_mut().unwrap().remove("metadata").unwrap();
        assert_eq!(metadata["rubocop_version"], "1.89.0");
        assert_eq!(metadata["ruby_engine"], "sonicop");
        assert_eq!(metadata["ruby_version"], "n/a");
        assert_eq!(
            report,
            serde_json::json!({"files": case["files"], "summary": case["summary"]}),
            "{}",
            case["name"]
        );
        assert_eq!(std::fs::read(&source_path).unwrap(), source);
        assert_eq!(std::fs::read(&config_path).unwrap(), config.as_bytes());
        assert_eq!(
            [
                std::fs::metadata(&source_path).unwrap().modified().unwrap(),
                std::fs::metadata(&config_path).unwrap().modified().unwrap(),
            ],
            modified,
            "{}",
            case["name"]
        );
    }
}
