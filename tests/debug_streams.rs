//! 本家の共通event列とnative固有の構成情報を分け、debugのredirectを検査する。

mod support;

use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::{Duration, UNIX_EPOCH};

use regex::Regex;
use serde_json::{Value, json};
use support::project::command;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/debug_streams.json")).unwrap()
}

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn native_text(text: &str) -> String {
    if cfg!(windows) {
        text.to_owned()
    } else {
        // 参照はWindows text mode。診断フィールドや入力bytesはこの変換へ渡さない。
        text.replace("\r\n", "\n")
    }
}

fn timer(text: &str) -> (String, usize) {
    let pattern =
        Regex::new(r"Finished in [0-9]+(?:\.[0-9]+)?(?:e[+-]?\d+)? seconds(\r?\n)\z").unwrap();
    if let Some(captures) = pattern.captures(text) {
        let complete = captures.get(0).unwrap();
        let mut result = text[..complete.start()].to_owned();
        result.push_str("Finished in <elapsed> seconds");
        result.push_str(captures.get(1).unwrap().as_str());
        (result, 1)
    } else {
        (text.to_owned(), 0)
    }
}

fn report(value: &mut Value) {
    let metadata = value.as_object_mut().unwrap().remove("metadata").unwrap();
    // 共通JSONからidentityだけを外す。Ruby版やGemパスをnativeへ捏造しない。
    assert_eq!(metadata["rubocop_version"], "1.89.0");
    assert_eq!(metadata["ruby_engine"], "sonicop");
    assert_eq!(metadata["ruby_version"], "n/a");
    assert!(
        metadata["sonicop_version"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
    );
}

fn project(data: &[u8], cwd: &Path, header: Option<u64>) -> Value {
    let mut text = String::from_utf8(data.to_vec()).unwrap();
    if let Some(targets) = header {
        // nativeの3行は共通payloadへ偽装しない。位置・実discovery件数・項目を別に検証する。
        let lines: Vec<&str> = text.splitn(4, '\n').collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(
            lines[0].starts_with("For ") && lines[0].contains(": configuration from "),
            "{text}"
        );
        let count = Regex::new(r", ([0-9]+) target\(s\), parallel=(?:true|false)\r?$")
            .unwrap()
            .captures(lines[0])
            .unwrap();
        assert_eq!(count[1].parse::<u64>().unwrap(), targets, "{text}");
        assert!(lines[1].starts_with("Implemented cops: "), "{text}");
        assert!(lines[2].starts_with("Unimplemented cops: "), "{text}");
        text = lines[3].to_owned();
    } else {
        assert!(!text.starts_with("For "), "{text}");
    }
    let (mut text, timer_count) = timer(&text);
    let mut rendered = String::new();
    let mut reports = Vec::new();
    while let Some(start) = text.find("{\"metadata\":") {
        let mut decoder = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        let mut value = decoder.next().unwrap().unwrap();
        let length = decoder.byte_offset();
        report(&mut value);
        reports.push(value);
        rendered.push_str(&text[..start]);
        rendered.push_str(&format!("<JSON[{}]>", reports.len() - 1));
        text = text[start + length..].to_owned();
    }
    rendered.push_str(&text);
    // JSONのmessage/locationを外へ退避してから、streamの動的なcwdだけを投影する。
    let cwd = cwd.to_string_lossy().replace('\\', "/");
    json!({"text": rendered.replace(&cwd, "<cwd>"), "reports": reports, "timer_count": timer_count})
}

fn expected(projection: &Value) -> Value {
    json!({
        "text": native_text(projection["text"].as_str().unwrap()),
        "reports": projection["reports"],
        "timer_count": projection["timer_count"],
    })
}

fn temporary_directory() -> tempfile::TempDir {
    let parent = std::env::temp_dir();
    // macOSの/varと/private/varを作成時に揃え、出力pathを一般的に正規化しない。
    #[cfg(target_os = "macos")]
    let parent = fs::canonicalize(parent).unwrap();
    tempfile::Builder::new()
        .prefix("sonicop-debug-")
        .tempdir_in(parent)
        .unwrap()
}

fn run(fixture: &Value, case: &Value) -> (tempfile::TempDir, Output) {
    let directory = temporary_directory();
    fs::write(
        directory.path().join("config.yml"),
        fixture["config"].as_str().unwrap(),
    )
    .unwrap();
    for (name, source) in case["inputs"].as_object().unwrap() {
        let path = directory.path().join(name);
        fs::write(&path, source.as_str().unwrap()).unwrap();
        // 120のmtime順差をdebugのfile event順へ混ぜない。
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            )
            .unwrap();
    }
    let mut run = command(directory.path());
    for name in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        run.env_remove(name);
    }
    run.args(
        case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap()),
    );
    run.write_stdin(case["stdin_source"].as_str().unwrap());
    let output = run.output().unwrap();
    assert_eq!(
        output.status.code(),
        case["exit"].as_i64().map(|code| code as i32),
        "{}: {:?}",
        case["id"],
        output.stderr
    );
    for (name, source) in case["inputs"].as_object().unwrap() {
        assert_eq!(
            fs::read(directory.path().join(name)).unwrap(),
            source.as_str().unwrap().as_bytes(),
            "{} / {name}",
            case["id"]
        );
    }
    (directory, output)
}

fn assert_outputs(directory: &Path, output: &Output, case: &Value, target_count: u64) {
    let header = case["debug"].as_bool().unwrap().then_some(target_count);
    assert_eq!(
        project(&output.stdout, directory, header),
        expected(&case["stdout"]),
        "{} stdout",
        case["id"]
    );
    assert_eq!(
        project(&output.stderr, directory, None),
        expected(&case["stderr"]),
        "{} stderr",
        case["id"]
    );
    for (name, reference) in case["outputs"].as_object().unwrap() {
        let path = directory.join(name);
        if reference.is_null() {
            assert!(!path.exists(), "{} / {name}", case["id"]);
        } else if reference["report"].is_object() {
            let mut actual: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            report(&mut actual);
            assert_eq!(actual, reference["report"], "{} / {name}", case["id"]);
        } else {
            let raw = String::from_utf8(bytes(reference["raw_hex"].as_str().unwrap())).unwrap();
            assert_eq!(
                fs::read(path).unwrap(),
                native_text(&raw).as_bytes(),
                "{} / {name}",
                case["id"]
            );
        }
    }
}

#[test]
fn debug_replays_file_events_and_keeps_formatter_files_separate() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "interface")
    {
        count += 1;
        let (directory, output) = run(&fixture, case);
        assert_outputs(
            directory.path(),
            &output,
            case,
            case["target_count"].as_u64().unwrap(),
        );
    }
    assert_eq!(count, 21);
}

#[test]
fn debug_timer_guards_preserve_early_and_deferred_error_boundaries() {
    let fixture = fixture();
    let mut phase_guards = 0;
    let mut deferred = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] != "interface")
    {
        let (_directory, output) = run(&fixture, case);
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            timer(&stdout).1,
            case["stdout"]["timer_count"].as_u64().unwrap() as usize,
            "{}",
            case["id"]
        );
        assert_eq!(timer(&stderr).1, 0, "{}", case["id"]);
        if case["coverage"] == "deferred_error_residual" {
            assert_eq!(case["residual_issue"], 118);
            // fixtureの旧分類を保持する。共通payloadは別テストで全項目を検査する。
            deferred += 1;
        } else {
            phase_guards += 1;
        }
        if case["stdout"]["timer_count"] == 0 {
            assert!(stdout.is_empty(), "{}: {stdout}", case["id"]);
        }
    }
    assert_eq!(phase_guards, 5);
    assert_eq!(deferred, 1);
}

#[test]
fn debug_rejection_keeps_the_first_file_event_and_empty_report() {
    let fixture = fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "unknown_cop")
        .unwrap();
    let (directory, output) = run(&fixture, case);
    assert_outputs(directory.path(), &output, case, 1);
}

#[test]
fn debug_directory_stdin_uses_discovery_counts_and_rejection_events() {
    let document: Value =
        serde_json::from_str(include_str!("fixtures/debug_directories.json")).unwrap();
    let cases = document["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases {
        let directory = temporary_directory();
        let config = document["config"].as_str().unwrap();
        fs::write(directory.path().join("config.yml"), config).unwrap();
        let root = directory.path().join("project");
        fs::create_dir(&root).unwrap();
        let mut before = Vec::new();
        for (name, source) in case["inputs"].as_object().unwrap() {
            let path = root.join(name);
            fs::write(&path, source.as_str().unwrap()).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(
                    UNIX_EPOCH + Duration::from_nanos(case["mtime_ns"][name].as_u64().unwrap()),
                ))
                .unwrap();
            before.push((
                name.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            ));
        }
        let mut command = command(&root);
        for name in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(name);
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        command.write_stdin(case["stdin_source"].as_str().unwrap());
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32)
        );
        let expected = json!({
            "id": case["id"], "debug": true,
            "stdout": case["stdout"], "stderr": case["stderr"],
            "outputs": {"report.out": case["output"], "other.out": null},
        });
        assert_outputs(
            &root,
            &output,
            &expected,
            case["target_count"].as_u64().unwrap(),
        );
        for (name, bytes, modified) in before {
            let path = root.join(name);
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
        assert_eq!(
            fs::read(directory.path().join("config.yml")).unwrap(),
            config.as_bytes()
        );
    }
}
