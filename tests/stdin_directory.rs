//! 本家の対象展開・順序・共有stdin本文を、実CLI記録と照合する。

use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/stdin_directory_reports.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 29);
    let mut names: Vec<_> = cases
        .iter()
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 29);
    for (kind, count) in [("json", 23), ("list", 2), ("payload", 4)] {
        assert_eq!(
            cases.iter().filter(|case| case["kind"] == kind).count(),
            count
        );
    }
    fixture
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    let (pairs, remainder) = text.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn project(case: &Value) -> tempfile::TempDir {
    // macOS の /var は /private/var の別名。絶対入力と子の cwd を同じ表記で作る。
    // 保存した診断の path を正規化せず、参照記録と同じ実行条件を保つ。
    #[cfg(target_os = "macos")]
    let directory = tempfile::Builder::new()
        .tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    #[cfg(not(target_os = "macos"))]
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join(".rubocop.yml"), case["config"].as_str().unwrap()).unwrap();
    if let Some(configs) = case["configs"].as_object() {
        for (path, text) in configs {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text.as_str().unwrap()).unwrap();
        }
    }
    for (path, content) in case["files"].as_object().unwrap() {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes(content)).unwrap();
    }
    if let Some(mtimes) = case["mtimes"].as_object() {
        for (path, timestamp) in mtimes {
            let modified = UNIX_EPOCH + Duration::from_secs(timestamp.as_u64().unwrap());
            fs::File::options()
                .write(true)
                .open(root.join(path))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
    }
    directory
}

fn stdout_template(stdout: &[u8], root: &std::path::Path) -> Vec<u8> {
    let boundary = stdout
        .windows(20)
        .position(|bytes| bytes == b"====================")
        .unwrap_or(stdout.len());
    let prefix = format!("{}/", root.to_string_lossy().replace('\\', "/"));
    let mut template = Vec::new();
    // 実行時に作った既知rootの行頭だけを置換し、payload内の同じ文字列へ触れない。
    for line in stdout[..boundary].split_inclusive(|&byte| byte == b'\n') {
        if let Some(suffix) = line.strip_prefix(prefix.as_bytes()) {
            template.extend_from_slice(b"<WORKDIR>/");
            template.extend_from_slice(suffix);
        } else {
            template.extend_from_slice(line);
        }
    }
    template.extend_from_slice(&stdout[boundary..]);
    template
}

fn run(case: &Value) -> Output {
    let directory = project(case);
    let root = directory.path();
    let supplied = case["stdin"].as_str().unwrap();
    let stdin_path = if supplied == "<ABS_SRC>" {
        root.join("src")
    } else {
        supplied.into()
    };
    let format = if case["kind"] == "payload" {
        "files"
    } else {
        "json"
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(root)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(["--cache", "false", "-f", format]);
    if !case["args"]
        .as_array()
        .unwrap()
        .iter()
        .any(|arg| arg == "--only")
    {
        command.args(["--only", "Layout/SpaceAroundOperators"]);
    }
    for arg in case["args"].as_array().unwrap() {
        command.arg(arg.as_str().unwrap());
    }
    let mut child = command
        .arg("--stdin")
        .arg(stdin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&bytes(&case["source_hex"]))
        .unwrap();
    let mut output = child.wait_with_output().unwrap();
    for (path, content) in case["files"].as_object().unwrap() {
        assert_eq!(fs::read(root.join(path)).unwrap(), bytes(content), "{case}");
    }
    assert_eq!(
        output.status.code(),
        Some(case["expected"]["exit"].as_i64().unwrap() as i32),
        "{}: {}",
        case["name"],
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "{case}: {output:?}");
    if case["kind"] == "payload" {
        output.stdout = stdout_template(&output.stdout, root);
    }
    output
}

#[test]
fn stdin_directories_match_reference_reports_and_leave_disk_unchanged() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        if case["kind"] != "json" {
            continue;
        }
        let output = run(case);
        let mut report: Value = serde_json::from_slice(&output.stdout).unwrap();
        report.as_object_mut().unwrap().remove("metadata");
        assert_eq!(report, case["expected"]["report"], "{}", case["name"]);
    }
}

#[test]
fn stdin_directory_listing_uses_discovery_order() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        if case["kind"] != "list" {
            continue;
        }
        let output = run(case);
        let text = String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n");
        let paths: Vec<_> = text.lines().collect();
        let expected: Vec<_> = case["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect();
        assert_eq!(paths, expected, "{}", case["name"]);
    }
}

#[test]
fn stdin_directory_payload_is_the_last_shared_body_even_without_targets() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        if case["kind"] != "payload" {
            continue;
        }
        let output = run(case);
        #[cfg(windows)]
        assert_eq!(output.stdout, bytes(&case["expected"]["stdout_hex"]));
        let separator = b"====================";
        let boundaries: Vec<_> = output
            .stdout
            .windows(separator.len())
            .enumerate()
            .filter_map(|(index, token)| (token == separator).then_some(index))
            .collect();
        assert_eq!(boundaries.len(), 1, "{}", case["name"]);
        let payload = &output.stdout[boundaries[0] + separator.len()..];
        // レポートの行末とは分け、改行を持たない本文の実バイトを厳密に検査する。
        let payload = payload
            .strip_prefix(b"\r\n")
            .or_else(|| payload.strip_prefix(b"\n"))
            .unwrap();
        assert_eq!(payload, bytes(&case["payload_hex"]), "{}", case["name"]);
    }
}

#[cfg(windows)]
fn assert_recorded_echo(case: &Value, existing: &Value) {
    if let Some(binding) = case["binding"].as_str() {
        let recorded = ["safety_cases", "literal_boundary_cases"]
            .into_iter()
            .flat_map(|key| existing[key].as_array().unwrap())
            .find(|record| record["name"] == binding)
            .unwrap();
        assert_eq!(case["source_hex"], recorded["source_hex"]);
        assert_eq!(case["safe_payload_hex"], recorded["safe"]["stdout_hex"]);
        assert_eq!(case["safe_exit"], recorded["safe"]["exit"]);
    }
    let directory = project(case);
    let root = directory.path();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(root)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args([
            "--cache",
            "false",
            "--only",
            case["only"].as_str().unwrap(),
            "-f",
            "json",
        ]);
    if case["list_only"] == true {
        // 一覧表示でも本家は元の stdin を echo し、検査や補正自体は行わない。
        command.args(["-f", "files", "--list-target-files"]);
    } else if case["json_only"] != true {
        command.args(["-o", "report.json", "-f", "files"]);
    }
    let mut child = command
        .args([
            case["mode"].as_str().unwrap(),
            "--stdin",
            case["stdin_name"].as_str().unwrap_or("src"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&bytes(&case["source_hex"]))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(case["expected"]["exit"].as_i64().unwrap() as i32),
        "{}: {}",
        case["name"],
        String::from_utf8_lossy(&output.stderr)
    );
    let mut report = if case["list_only"] == true {
        assert_eq!(
            stdout_template(&output.stdout, root),
            bytes(&case["expected"]["stdout_hex"])
        );
        Value::Null
    } else {
        let report_bytes = if case["json_only"] == true {
            output.stdout.clone()
        } else {
            fs::read(root.join("report.json")).unwrap()
        };
        serde_json::from_slice(&report_bytes).unwrap()
    };
    if let Some(report) = report.as_object_mut() {
        report.remove("metadata");
    }
    assert_eq!(report, case["expected"]["report"], "{}", case["name"]);
    if let Some(values) = case["expected"]["values"].as_object() {
        assert_eq!(values["original"], values["safe"], "{}", case["name"]);
        if case["policy"] == "preserve-tail" || case["policy"] == "original-encoding" {
            assert_ne!(values["original"], values["reference"]);
        } else {
            assert_eq!(values["original"], values["reference"]);
        }
    }
    if case["expected"]["exit"] == 2 {
        let reason = String::from_utf8_lossy(&output.stderr);
        assert!(
            reason.contains("Autocorrection was not written")
                || reason.contains("refusing to print corrected stdin"),
            "{case}: {reason}"
        );
        assert!(
            report["files"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|file| file["offenses"].as_array().unwrap())
                .all(|offense| offense["corrected"] == false)
        );
    } else {
        assert!(output.stderr.is_empty(), "{case}: {output:?}");
    }
    if case["json_only"] != true {
        assert_eq!(
            stdout_template(&output.stdout, root),
            bytes(&case["expected"]["stdout_hex"]),
            "{}",
            case["name"]
        );
        let separator = b"====================";
        let boundaries: Vec<_> = output
            .stdout
            .windows(separator.len())
            .enumerate()
            .filter_map(|(index, token)| (token == separator).then_some(index))
            .collect();
        assert_eq!(boundaries.len(), 1, "{}", case["name"]);
        let payload = &output.stdout[boundaries[0] + separator.len()..];
        let payload = payload
            .strip_prefix(b"\r\n")
            .or_else(|| payload.strip_prefix(b"\n"))
            .unwrap();
        assert_eq!(
            payload,
            bytes(&case["expected"]["payload_hex"]),
            "{}",
            case["name"]
        );
    }
    for (path, content) in case["files"].as_object().unwrap() {
        assert_eq!(fs::read(root.join(path)).unwrap(), bytes(content), "{case}");
    }
}

#[cfg(windows)]
#[test]
fn stdin_directory_echo_is_planned_once_and_rolls_back_the_whole_buffer() {
    let fixture = fixture();
    let cases = fixture["echo_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 22);
    let existing: Value =
        serde_json::from_str(include_str!("fixtures/windows_stdio.json")).unwrap();
    for case in cases
        .iter()
        .filter(|case| case["policy"] != "original-encoding")
    {
        assert_recorded_echo(case, &existing);
    }
}

#[cfg(windows)]
#[test]
fn windows1252_stdin_success_keeps_the_declared_encoding_and_original_value() {
    let fixture = fixture();
    let cases: Vec<_> = fixture["echo_cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["policy"] == "original-encoding")
        .collect();
    assert_eq!(cases.len(), 2);
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let entries = manifest["windows_stdin_encoding_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["kind"], "original-encoding");
    assert_eq!(entry["platform"], "windows");
    assert_eq!(entry["cop"], "Layout/SpaceAroundOperators");
    assert_eq!(entry["modes"].as_sequence().unwrap().len(), 2);
    assert_eq!(entry["target_engines"].as_sequence().unwrap().len(), 1);
    assert_eq!(entry["target_engines"][0], "2.7/parser_whitequark");
    let existing: Value =
        serde_json::from_str(include_str!("fixtures/windows_stdio.json")).unwrap();
    for case in cases {
        assert_eq!(
            entry["source_hex"].as_str().unwrap(),
            case["source_hex"].as_str().unwrap()
        );
        assert_eq!(case["reference"]["exit"], 0);
        assert_eq!(case["expected"]["exit"], 0);
        assert_eq!(
            entry["upstream"]["payload_hex"].as_str().unwrap(),
            case["reference"]["payload_hex"].as_str().unwrap()
        );
        assert_eq!(
            entry["sonicop"]["payload_hex"].as_str().unwrap(),
            case["expected"]["payload_hex"].as_str().unwrap()
        );
        assert!(
            entry["modes"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|mode| mode.as_str() == case["mode"].as_str())
        );
        for (side, name) in [
            ("original", "original"),
            ("upstream", "reference"),
            ("sonicop", "safe"),
        ] {
            let value: Value =
                serde_json::from_slice(&bytes(&case["expected"]["values"][name]["stdout_hex"]))
                    .unwrap();
            assert_eq!(
                entry[side]["value_bytes"],
                serde_yaml_ng::to_value(&value["bytes"]).unwrap()
            );
            assert_eq!(
                entry[side]["value_encoding"].as_str().unwrap(),
                value["encoding"].as_str().unwrap()
            );
        }
        let upstream = bytes(&case["reference"]["payload_hex"]);
        let safe = bytes(&case["expected"]["payload_hex"]);
        assert!(!upstream.contains(&0xe9));
        assert!(upstream.windows(2).any(|pair| pair == [0xc3, 0xa9]));
        assert!(safe.contains(&0xe9));
        assert_recorded_echo(case, &existing);
    }
}
