//! fail-fast の引数 group と directory 内順を、実 RuboCop の記録で検査する。

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::tempdir;

fn bytes(hex: &Value) -> Vec<u8> {
    let (pairs, remainder) = hex.as_str().unwrap().as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn stamp(root: &Path, case: &Value) -> Vec<(Vec<u8>, u64, std::time::SystemTime)> {
    case["files"]
        .as_object()
        .unwrap()
        .keys()
        .map(|name| {
            let path = root.join(name);
            let metadata = fs::metadata(&path).unwrap();
            (
                fs::read(path).unwrap(),
                metadata.len(),
                metadata.modified().unwrap(),
            )
        })
        .collect()
}

fn prepare(root: &Path, case: &Value) {
    for (name, hex) in case["files"].as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes(hex)).unwrap();
        let ns = case["mtime_ns"][name].as_i64().unwrap();
        let duration = Duration::from_nanos(ns.unsigned_abs());
        let modified = if ns < 0 {
            UNIX_EPOCH - duration
        } else {
            UNIX_EPOCH + duration
        };
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }
}

fn arguments(case: &Value) -> Vec<String> {
    let mut args = vec![
        "--config".into(),
        "../config.yml".into(),
        "--cache".into(),
        case["cached"].as_bool().unwrap().to_string(),
        "--only".into(),
        "Layout/SpaceAroundOperators".into(),
    ];
    if case["fail_fast"].as_bool().unwrap_or(true) {
        args.push("--fail-fast".into());
    }
    if case["cached"] == true {
        args.extend(["--cache-root".into(), "../cache".into()]);
    }
    if !case["stdin_hex"].is_null() {
        args.extend([
            "--stdin".into(),
            case["targets"][0].as_str().unwrap().into(),
        ]);
    } else {
        args.extend(
            case["targets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().into()),
        );
    }
    args
}

fn assert_case(root: &Path, case: &Value) {
    let name = case["name"].as_str().unwrap();
    let before = stamp(root, case);
    let args = arguments(case);
    let mut inspect = cargo_bin_cmd!("sonicop");
    inspect
        .current_dir(root)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(&args)
        .args(["--format", "json"]);
    if !case["stdin_hex"].is_null() {
        inspect.write_stdin(bytes(&case["stdin_hex"]));
    }
    let output = inspect.output().unwrap();
    assert_eq!(
        output.status.code(),
        case["exit"].as_i64().map(|v| v as i32),
        "{name}"
    );
    assert_eq!(output.stderr, bytes(&case["stderr_hex"]), "{name}");
    let mut stream = serde_json::Deserializer::from_slice(&output.stdout).into_iter::<Value>();
    let mut report = stream.next().unwrap().unwrap();
    // 実装名・実行環境だけを除き、file順・全診断field・summaryを保持する。
    report.as_object_mut().unwrap().remove("metadata");
    assert_eq!(report, case["report"], "{name}");
    let tail = std::str::from_utf8(&output.stdout[stream.byte_offset()..]).unwrap();
    // 構成したASCII入力の transport CRLF だけを投影し、順序の試験と分ける。
    assert_eq!(
        tail.replace("\r\n", "\n"),
        case["after_json"].as_str().unwrap().replace("\r\n", "\n"),
        "{name}"
    );
    let mut listing = cargo_bin_cmd!("sonicop");
    listing
        .current_dir(root)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(&args)
        .arg("-L");
    if !case["stdin_hex"].is_null() {
        listing.write_stdin(bytes(&case["stdin_hex"]));
    }
    let output = listing.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{name}");
    assert!(output.stderr.is_empty(), "{name}");
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        case["listing"].as_str().unwrap().replace("\r\n", "\n"),
        "{name}"
    );
    assert_eq!(stamp(root, case), before, "{name}");
}

#[test]
fn disk_arguments_and_directory_groups_follow_ruby_order() {
    let document: Value =
        serde_json::from_str(include_str!("fixtures/fail_fast_order.json")).unwrap();
    let mut groups = HashMap::new();
    for case in document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["stdin_hex"].is_null())
    {
        if case["windows_only"] == true && !cfg!(windows) {
            continue;
        }
        let group = case["group"].as_str().unwrap();
        if !groups.contains_key(group) {
            let project = tempdir().unwrap();
            let root = project.path().join("project");
            fs::create_dir(&root).unwrap();
            fs::write(
                project.path().join("config.yml"),
                case["config"]
                    .as_str()
                    .unwrap_or(document["config"].as_str().unwrap()),
            )
            .unwrap();
            prepare(&root, case);
            groups.insert(group.to_owned(), project);
        } else {
            // warmでは同じpath・原本mtime・設定とcacheを保ち、ファイルを作り直さない。
            assert_eq!(case["repeat"], 1);
            let cache = groups[group].path().join("cache");
            assert!(cache.exists());
            assert!(fs::read_dir(cache).unwrap().next().is_some());
        }
        let project = &groups[group];
        let root = project.path().join("project");
        assert_case(&root, case);
    }
}

#[test]
fn stdin_directory_uses_the_same_ruby_order() {
    let document: Value =
        serde_json::from_str(include_str!("fixtures/fail_fast_order.json")).unwrap();
    for case in document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| !case["stdin_hex"].is_null())
    {
        if case["windows_only"] == true && !cfg!(windows) {
            continue;
        }
        let project = tempdir().unwrap();
        let root = project.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(
            project.path().join("config.yml"),
            document["config"].as_str().unwrap(),
        )
        .unwrap();
        prepare(&root, case);
        assert_case(&root, case);
    }
}

#[test]
fn rejected_selection_listing_uses_the_same_directory_order() {
    let document: Value =
        serde_json::from_str(include_str!("fixtures/fail_fast_rejected_order.json")).unwrap();
    let cases = document["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let project = tempdir().unwrap();
        let root = project.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(
            project.path().join("config.yml"),
            document["config"].as_str().unwrap(),
        )
        .unwrap();
        prepare(&root, case);
        let before = stamp(&root, case);
        let mut command = cargo_bin_cmd!("sonicop");
        command.current_dir(&root);
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        );
        if !case["stdin_hex"].is_null() {
            command.write_stdin(bytes(&case["stdin_hex"]));
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32)
        );
        assert_eq!(output.stderr, bytes(&case["stderr_hex"]));
        let expected = bytes(&case["stdout_hex"]);
        if cfg!(windows) {
            assert_eq!(output.stdout, expected);
        } else {
            // 構成したASCII一覧だけの輸送差を投影し、本文を含むstreamへ広げない。
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .replace("\r\n", "\n"),
                String::from_utf8(expected).unwrap().replace("\r\n", "\n")
            );
        }
        assert_eq!(stamp(&root, case), before);
        assert_eq!(
            fs::read(project.path().join("config.yml")).unwrap(),
            document["config"].as_str().unwrap().as_bytes()
        );
    }
}
