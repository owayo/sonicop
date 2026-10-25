use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("config.yml"),
        b"AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.3\n",
    )
    .unwrap();
    fs::write(directory.path().join("guard.rb"), b"guard=1\r\n").unwrap();
    directory
}

fn run(root: &Path, origin: &str, options: &[u8], arguments: &[&str], source: &[u8]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(root)
        .env_remove("RUBOCOP_OPTS")
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if origin == "dotfile" {
        fs::write(root.join(".rubocop"), options).unwrap();
    } else {
        command.env("RUBOCOP_OPTS", std::str::from_utf8(options).unwrap());
    }
    let mut child = command.spawn().unwrap();
    if let Err(error) = child.stdin.take().unwrap().write_all(source) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    child.wait_with_output().unwrap()
}

#[test]
fn ruby_whitespace_separates_options_from_both_sources() {
    for origin in ["dotfile", "environment"] {
        for separator in ["\r\n", "\r", "\u{b}", "\u{c}"] {
            let directory = project();
            let options =
                format!("--format{separator}json{separator}--stdin{separator}'日本 😀.rb'");
            let output = run(
                directory.path(),
                origin,
                options.as_bytes(),
                &[
                    "--config",
                    "config.yml",
                    "--cache",
                    "false",
                    "--only",
                    "Layout/SpaceAroundOperators",
                ],
                b"x=1\r\n",
            );
            assert_eq!(output.status.code(), Some(1), "{origin}, {separator:?}");
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["summary"]["offense_count"], 1);
            assert_eq!(report["files"][0]["path"], "日本 😀.rb");
        }
    }
}

#[cfg(windows)]
#[test]
fn external_options_match_recorded_windows_rubocop_reports() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/external_options_windows.json")).unwrap();
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 47);
    for case in cases {
        let directory = project();
        let options = decode(case["options_hex"].as_str().unwrap());
        let arguments: Vec<_> = case["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|argument| argument.as_str().unwrap())
            .collect();
        let output = run(
            directory.path(),
            case["origin"].as_str().unwrap(),
            &options,
            &arguments,
            &decode(case["stdin_hex"].as_str().unwrap()),
        );
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{}: {}",
            case["id"],
            String::from_utf8_lossy(&output.stderr)
        );
        if !case["report"].is_null() {
            let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(actual["files"], case["report"]["files"], "{}", case["id"]);
            assert_eq!(
                actual["summary"], case["report"]["summary"],
                "{}",
                case["id"]
            );
        } else {
            // 既存の引数エラーの文面差は固定せず、通常の終了 2 と原本保持を監視する。
            assert_eq!(
                output.stdout.is_empty(),
                case["stdout_empty"].as_bool().unwrap()
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
        }
        assert_eq!(
            fs::read(directory.path().join("guard.rb")).unwrap(),
            b"guard=1\r\n"
        );
        if case["origin"] == "dotfile" {
            assert_eq!(
                fs::read(directory.path().join(".rubocop")).unwrap(),
                options
            );
        }
    }
}

#[test]
fn control_whitespace_keeps_dotfile_environment_command_line_order() {
    let directory = project();
    let root = directory.path();
    fs::write(root.join("対象 日本😀.rb"), b"x=1\r\n").unwrap();
    fs::write(
        root.join(".rubocop"),
        b"--config config.yml --cache false --only Layout/SpaceAroundOperators --format json\r\n",
    )
    .unwrap();
    // env の --out は dotfile の JSON へ、CLI の --out は env の simple へ結び付く。
    let output = run(
        root,
        "environment",
        "--out 'environment 日本.json'\u{b}--format\u{c}simple".as_bytes(),
        &["--out", "command 日本.txt", "対象 日本😀.rb"],
        b"",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("environment 日本.json")).unwrap()).unwrap();
    assert_eq!(json["files"][0]["path"], "対象 日本😀.rb");
    assert_eq!(json["summary"]["offense_count"], 1);
    assert!(
        fs::read_to_string(root.join("command 日本.txt"))
            .unwrap()
            .contains("Layout/SpaceAroundOperators")
    );
    assert_eq!(fs::read(root.join("対象 日本😀.rb")).unwrap(), b"x=1\r\n");
}
