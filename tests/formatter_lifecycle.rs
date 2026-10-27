//! FormatterSet の共有 stream と、独立した同一 File handle の実 CLI 記録を照合する。
use std::{
    fs,
    io::Write,
    process::{Command, Output, Stdio},
};

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

fn native_record(hex: &Value) -> Vec<u8> {
    let bytes = bytes(hex);
    if cfg!(windows) {
        bytes
    } else {
        String::from_utf8(bytes)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn run(project: &std::path::Path, case: &Value, paths: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(project)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .env_remove("RUBOCOP_OPTS")
        .env_remove("RUBY_PROGRESS_BAR_LENGTH")
        .args([
            "--config",
            "config.yml",
            "--cache",
            "false",
            "--only",
            "Layout/SpaceAroundOperators",
            "--no-color",
        ]);
    for flag in case["flags"].as_array().unwrap() {
        command.arg(flag.as_str().unwrap());
    }
    for pair in case["formats"].as_array().unwrap() {
        command.args(["--format", pair[0].as_str().unwrap()]);
        if let Some(path) = pair[1].as_str() {
            command.args(["--out", path]);
        }
    }
    if case["stdin_hex"].is_string() {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&bytes(&case["stdin_hex"]))
            .unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.args(paths).output().unwrap()
    }
}

fn without_timer(bytes: Vec<u8>) -> Vec<u8> {
    let text = String::from_utf8(bytes).unwrap();
    let (body, timer) = text.rsplit_once("Finished in ").unwrap();
    assert!(!body.contains("Finished in "));
    let timer = timer
        .strip_suffix(if cfg!(windows) {
            " seconds\r\n"
        } else {
            " seconds\n"
        })
        .unwrap();
    assert!(timer.parse::<f64>().unwrap() >= 0.0);
    assert!(!timer.contains(['\r', '\n']));
    body.as_bytes().to_vec()
}

#[test]
fn formatter_callbacks_share_streams_in_reference_event_order() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_lifecycle.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 12);
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        // 同一 File の上書き位置は物理 CRLF byte 数にも依存する。Unix の新しい期待へ投影しない。
        if !cfg!(windows) && name.starts_with("same-file-") {
            continue;
        }
        let project = tempfile::tempdir().unwrap();
        fs::write(
            project.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let source = b"x=1\n".repeat(case["source_bytes"].as_u64().unwrap() as usize / 4);
        fs::write(project.path().join("a.rb"), &source).unwrap();
        fs::write(project.path().join("b.rb"), b"y = 2\n").unwrap();
        if let Some(times) = case["mtimes_unix_seconds"].as_object() {
            // #120 の既存優先順差を、このcallback回帰の入力metadataへ混ぜない。
            for (name, seconds) in times {
                let time = std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(seconds.as_u64().unwrap());
                fs::OpenOptions::new()
                    .write(true)
                    .open(project.path().join(name))
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(time))
                    .unwrap();
            }
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(project.path())
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBY_PROGRESS_BAR_LENGTH")
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--only",
                "Layout/SpaceAroundOperators",
                "--no-color",
            ]);
        for flag in case["flags"].as_array().unwrap() {
            command.arg(flag.as_str().unwrap());
        }
        for pair in case["formats"].as_array().unwrap() {
            command.args(["--format", pair[0].as_str().unwrap()]);
            if let Some(path) = pair[1].as_str() {
                command.args(["--out", path]);
            }
        }
        let actual = if let Some(paths) = case["paths"].as_array() {
            command
                .args(paths.iter().map(|path| path.as_str().unwrap()))
                .output()
                .unwrap()
        } else {
            command.args(["a.rb", "b.rb"]).output().unwrap()
        };
        let expected = &case["observation"];
        let mut matches = actual.status.code() == expected["exit"].as_i64().map(|code| code as i32)
            && actual.stdout == native_record(&expected["stdout_hex"])
            && actual.stderr == native_record(&expected["stderr_hex"]);
        for (path, expected) in case["files_hex"].as_object().unwrap() {
            matches &= fs::read(project.path().join(path)).unwrap() == native_record(expected);
        }
        assert_eq!(
            fs::read(project.path().join("a.rb")).unwrap(),
            source,
            "{name}"
        );
        assert_eq!(
            fs::read(project.path().join("b.rb")).unwrap(),
            b"y = 2\n",
            "{name}"
        );
        if !matches {
            differences.push(name);
        }
    }
    assert!(
        differences.is_empty(),
        "formatter event 差異: {differences:?}"
    );
}

#[test]
fn elapsed_time_follows_all_formatters_and_the_final_stdin_echo_once() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_lifecycle.json")).unwrap();
    let cases = fixture["timer_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 5);
    for case in cases {
        let project = tempfile::tempdir().unwrap();
        fs::write(
            project.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(project.path().join("a.rb"), b"x=1\n").unwrap();
        fs::write(project.path().join("b.rb"), b"y = 2\n").unwrap();
        let actual = run(project.path(), case, &["a.rb", "b.rb"]);
        let expected = &case["observation"];
        assert_eq!(
            actual.status.code(),
            expected["exit"].as_i64().map(|code| code as i32),
            "{}",
            case["name"]
        );
        assert_eq!(
            without_timer(actual.stdout),
            without_timer(native_record(&expected["stdout_hex"])),
            "{}",
            case["name"]
        );
        assert_eq!(
            actual.stderr,
            native_record(&expected["stderr_hex"]),
            "{}",
            case["name"]
        );
        for (path, expected) in case["files_hex"].as_object().unwrap() {
            assert_eq!(
                fs::read(project.path().join(path)).unwrap(),
                native_record(expected),
                "{}",
                case["name"]
            );
        }
        assert_eq!(fs::read(project.path().join("a.rb")).unwrap(), b"x=1\n");
    }
    // debug も同じensureを有効にする。構成/走査ログのchannel修正は #122 に委ねる。
    let case = &fixture["debug_out_control"];
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("config.yml"),
        fixture["config"].as_str().unwrap(),
    )
    .unwrap();
    fs::write(project.path().join("a.rb"), b"x=1\n").unwrap();
    let actual = run(project.path(), case, &["a.rb"]);
    assert_eq!(
        actual.status.code(),
        case["exit"].as_i64().map(|code| code as i32)
    );
    without_timer(actual.stdout);
    assert_eq!(
        fs::read(project.path().join("report.txt")).unwrap(),
        native_record(&case["file_hex"])
    );
    let scopes = fixture["timer_scope_cases"].as_array().unwrap();
    assert_eq!(scopes.len(), 8);
    for case in scopes {
        let project = tempfile::tempdir().unwrap();
        fs::write(
            project.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(project.path().join("case.rb"), b"x=1\n").unwrap();
        let actual = Command::new(env!("CARGO_BIN_EXE_sonicop"))
            .current_dir(project.path())
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBY_PROGRESS_BAR_LENGTH")
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(
            actual.status.code(),
            case["exit"].as_i64().map(|code| code as i32),
            "{}",
            case["name"]
        );
        if case["timer_expected"] == true {
            without_timer(actual.stdout);
        } else {
            assert!(actual.stdout.is_empty(), "{}", case["name"]);
        }
        assert_eq!(fs::read(project.path().join("case.rb")).unwrap(), b"x=1\n");
    }
}

#[test]
fn formatter_resolution_follows_each_output_open_before_any_started_event() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_lifecycle.json")).unwrap();
    let cases = fixture["initialization_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let project = tempfile::tempdir().unwrap();
        fs::write(
            project.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(project.path().join("a.rb"), b"x=1\n").unwrap();
        fs::write(project.path().join("first.txt"), b"first-original\n").unwrap();
        fs::write(project.path().join("second.txt"), b"second-original\n").unwrap();
        let actual = Command::new(env!("CARGO_BIN_EXE_sonicop"))
            .current_dir(project.path())
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBY_PROGRESS_BAR_LENGTH")
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--only",
                "Layout/SpaceAroundOperators",
                "--no-color",
            ])
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .arg("a.rb")
            .output()
            .unwrap();
        assert_eq!(
            actual.status.code(),
            case["exit"].as_i64().map(|code| code as i32),
            "{name}"
        );
        // class loader とnative enumのエラー文言は別の既存差。副作用とroutingだけを照合する。
        assert!(
            String::from_utf8(actual.stderr)
                .unwrap()
                .contains("unknown_formatter"),
            "{name}"
        );
        if case["timer_expected"] == true {
            assert!(without_timer(actual.stdout).is_empty(), "{name}");
        } else {
            assert!(actual.stdout.is_empty(), "{name}");
        }
        for (path, expected) in case["files_hex"].as_object().unwrap() {
            assert_eq!(
                fs::read(project.path().join(path)).unwrap(),
                native_record(expected),
                "{name}: {path}"
            );
        }
        assert_eq!(fs::read(project.path().join("a.rb")).unwrap(), b"x=1\n");
    }
}

#[test]
fn output_open_failures_precede_started_and_keep_the_timer_on_stdout() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/formatter_lifecycle.json")).unwrap();
    let cases = fixture["failure_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 7);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        if !cfg!(windows) && name == "readonly-output" {
            continue;
        }
        let project = tempfile::tempdir().unwrap();
        fs::write(
            project.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(project.path().join("a.rb"), b"x=1\n").unwrap();
        fs::create_dir(project.path().join("directory-output")).unwrap();
        fs::write(project.path().join("first.txt"), b"old-output\n").unwrap();
        let readonly = project.path().join("readonly.txt");
        fs::write(&readonly, b"readonly-original\n").unwrap();
        let permissions = fs::metadata(&readonly).unwrap().permissions();
        if name == "readonly-output" {
            let mut changed = permissions.clone();
            changed.set_readonly(true);
            fs::set_permissions(&readonly, changed).unwrap();
        }
        let actual = Command::new(env!("CARGO_BIN_EXE_sonicop"))
            .current_dir(project.path())
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBY_PROGRESS_BAR_LENGTH")
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--only",
                "Layout/SpaceAroundOperators",
                "--no-color",
            ])
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .arg("a.rb")
            .output()
            .unwrap();
        fs::set_permissions(&readonly, permissions).unwrap();
        assert_eq!(
            actual.status.code(),
            case["exit"].as_i64().map(|code| code as i32),
            "{name}"
        );
        if name == "missing-parent-control" {
            assert!(actual.stdout.is_empty() && actual.stderr.is_empty());
            assert_eq!(
                fs::read(project.path().join("nested/deep/report.txt")).unwrap(),
                native_record(&case["created_output_hex"])
            );
        } else {
            assert!(!actual.stderr.is_empty(), "{name}");
            if name.starts_with("timed-") {
                assert!(without_timer(actual.stdout).is_empty(), "{name}");
            } else {
                assert!(actual.stdout.is_empty(), "{name}");
            }
        }
        if name == "second-open-directory-after-file" {
            assert!(
                fs::read(project.path().join("first.txt"))
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(fs::read(&readonly).unwrap(), b"readonly-original\n");
        assert_eq!(fs::read(project.path().join("a.rb")).unwrap(), b"x=1\n");
    }
}
