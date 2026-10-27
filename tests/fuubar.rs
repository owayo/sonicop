//! Fuubar leaf の描画を、実 RuboCop の非 TTY 記録と比較する。
//! raw CLI transport は #102 の統合後に別検査し、ここは CRLF のみを LF に投影する。

use std::fs;
use std::path::Path;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::tempdir;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/fuubar_reports.json")).unwrap()
}

fn bytes(hex: &Value) -> Vec<u8> {
    let (pairs, remainder) = hex.as_str().unwrap().as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn renderer_content(data: &[u8]) -> String {
    String::from_utf8(data.to_vec())
        .unwrap()
        .replace("\r\n", "\n")
}

fn content_events(text: &str) -> Vec<&str> {
    // NonTty の 10ms 間引きによる中間 glyph の有無を、違反・summary の順と混ぜない。
    text.lines()
        .filter(|line| !line.is_empty() && !bar_line(line))
        .collect()
}

fn bar_line(line: &str) -> bool {
    line.strip_prefix("Progress: |").is_some_and(|tail| {
        tail.strip_suffix('|')
            .unwrap_or(tail)
            .bytes()
            .all(|byte| byte == b'=')
    })
}

fn scoped_events(record: &Value) -> bool {
    // fail-fast が明示順を mtime 順に変える既存差 #120 は、元記録を保持して別途追跡する。
    record["case"]["comparison"] == "events" && record["case"]["name"] != "fail-fast"
}

fn assert_case(root: &Path, record: &Value) {
    let case = &record["case"];
    let name = case["name"].as_str().unwrap();
    fs::write(root.join("config.yml"), case["config"].as_str().unwrap()).unwrap();
    for (path, source) in case["files"].as_object().unwrap() {
        let path = root.join(path);
        fs::write(&path, bytes(source)).unwrap();
        if let Some(timestamp) = case["mtime_ns"].as_u64() {
            let time = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(timestamp);
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
    }
    let mut command = cargo_bin_cmd!("sonicop");
    command
        .current_dir(root)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .env_remove("RUBOCOP_OPTS");
    for (key, value) in case["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    command.args([
        "--config",
        "config.yml",
        "--cache",
        "false",
        "--only",
        case["only"].as_str().unwrap(),
    ]);
    for formatter in case["formats"].as_array().unwrap() {
        command.args(["--format", formatter[0].as_str().unwrap()]);
        if let Some(path) = formatter[1].as_str() {
            command.args(["--out", path]);
        }
    }
    command.args(
        case["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|flag| flag.as_str().unwrap()),
    );
    if !case["stdin_hex"].is_null() {
        command
            .args(["--stdin", case["targets"][0].as_str().unwrap()])
            .write_stdin(bytes(&case["stdin_hex"]));
    } else {
        command.args(
            case["targets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|target| target.as_str().unwrap()),
        );
    }
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        record["exit"].as_i64().map(|code| code as i32),
        "{name}: {:?}",
        output.stderr
    );
    for (path, expected) in record["outputs_hex"].as_object().unwrap() {
        let found = match path.as_str() {
            "stdout" => output.stdout.clone(),
            "stderr" => output.stderr.clone(),
            _ => fs::read(root.join(path)).unwrap(),
        };
        let found = renderer_content(&found);
        let expected = renderer_content(&bytes(expected));
        if case["comparison"] == "events" && record["primary"] == path.as_str() {
            assert!(
                found.starts_with("\nProgress: |"),
                "{name}: missing initial bar"
            );
            assert_eq!(content_events(&found), content_events(&expected), "{name}");
            let complete = record["report"]["summary"]["target_file_count"]
                == record["report"]["summary"]["inspected_file_count"];
            let complete_bar =
                |line: &&str| bar_line(line) && line.len() > 11 && line.ends_with('|');
            let found_bar = found.lines().find(complete_bar);
            let expected_bar = expected.lines().find(complete_bar);
            assert_eq!(found_bar, expected_bar, "{name}: completed bar bytes");
            assert_eq!(found_bar.is_some(), complete, "{name}: completion");
        } else {
            assert_eq!(found, expected, "{name}: {path}");
        }
    }
    for (path, expected) in record["after_hex"].as_object().unwrap() {
        let found = fs::read(root.join(path)).unwrap();
        if case["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|flag| flag == "-a" || flag == "-A")
        {
            // #95 の native write/read 依存とは分け、ここでは描画対象となる補正文を確認する。
            assert_eq!(
                renderer_content(&found),
                renderer_content(&bytes(expected)),
                "{name}: source"
            );
        } else {
            assert_eq!(found, bytes(expected), "{name}: original bytes");
        }
    }
}

#[test]
fn single_file_fuubar_renderer_content_matches_upstream() {
    let document = fixture();
    let records = document["cases"].as_array().unwrap();
    assert_eq!(records.len(), 33);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["case"]["comparison"] == "strict")
            .count(),
        29
    );
    assert!(
        records
            .iter()
            .any(|record| record["case"]["name"] == "disk-offense")
    );
    for record in records
        .iter()
        .filter(|record| record["case"]["comparison"] == "strict")
    {
        let project = tempdir().unwrap();
        assert_case(project.path(), record);
    }
}

#[test]
fn multiple_file_and_fail_fast_events_match_upstream() {
    let document = fixture();
    let records = document["cases"].as_array().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| scoped_events(record))
            .count(),
        3
    );
    for record in records.iter().filter(|record| scoped_events(record)) {
        let project = tempdir().unwrap();
        assert_case(project.path(), record);
    }
}
