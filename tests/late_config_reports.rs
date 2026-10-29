//! Runner開始前の設定拒否と、検査途中の終了reportを本家の生出力で区別する。

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap().as_bytes();
    let (pairs, rest) = text.as_chunks::<2>();
    assert!(rest.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn without_metadata(data: &[u8]) -> Vec<u8> {
    let marker = b"\"metadata\":{";
    let Some(start) = data.windows(marker.len()).position(|part| part == marker) else {
        return data.to_vec();
    };
    let field = b"\"files\":";
    let end = start
        + data[start..]
            .windows(field.len())
            .position(|part| part == field)
            .unwrap();
    assert_eq!(data[start - 1], b'{');
    let mut metadata = vec![b'{'];
    metadata.extend_from_slice(data[start..end].strip_suffix(b",").unwrap());
    metadata.push(b'}');
    let metadata: Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(metadata["metadata"]["rubocop_version"], "1.89.0");
    assert_eq!(metadata["metadata"]["ruby_engine"], "sonicop");
    let mut result = data[..start].to_vec();
    result.extend_from_slice(&data[end..]);
    result
}

fn template(data: &[u8], root: &Path) -> Vec<u8> {
    #[cfg(target_os = "macos")]
    let physical = fs::canonicalize(root).unwrap();
    #[cfg(target_os = "macos")]
    let root = physical.as_path();
    let prefix = format!("{}/", root.to_string_lossy().replace('\\', "/"));
    String::from_utf8(without_metadata(data))
        .unwrap()
        .replace(&prefix, "<WORKDIR>/")
        .into_bytes()
}

fn native(value: &Value) -> Vec<u8> {
    let raw = bytes(value);
    if cfg!(windows) {
        raw
    } else {
        // 全条件はechoへ到達しない。Ruby textmodeが出したreport行末だけをOSへ投影する。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

#[test]
fn late_config_failures_keep_finished_reports_and_only_the_completed_prefix_writes() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/late_config_reports.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 38);
    assert_eq!(fixture["schema"], 1);
    assert_eq!(
        cases
            .iter()
            .map(|case| case["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "single",
            "prefix-ambiguous",
            "prefix-unknown",
            "prefix-obsolete",
            "prefix-missing",
            "failed-first",
            "root-directory",
            "discovery-directory",
            "initial",
            "initial-out",
            "list-files",
            "list-directory",
            "failfast-offense",
            "failfast-clean",
            "correct-a",
            "correct-A",
            "correct-parallel",
            "cache-single",
            "cache-parallel",
            "multiple",
            "json-out",
            "stderr-progress",
            "stderr-single",
            "stderr-parallel",
            "stdin-single",
            "stdin-single-files",
            "stdin-directory-a",
            "stdin-directory-A",
            "stdin-directory-files",
            "force-exclusion",
            "ignore-parent",
            "recognized",
            "out-is-directory",
            "normal",
            "normal-correct",
            "color-json",
            "selection-config",
            "unvisited-config"
        ]
    );
    let mut differences = Vec::new();
    let mut executed = 0;
    let mut strict = 0;
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let coverage = match id {
            "correct-parallel" | "cache-parallel" | "stderr-parallel" | "unvisited-config" => {
                "deferred_158"
            }
            "out-is-directory" => "open_envelope",
            _ => "strict",
        };
        assert_eq!(case["coverage"], coverage, "{id}");
        if !cfg!(windows) && coverage == "deferred_158" {
            // 原4条件はWindowsのfork非対応fallback。未測定のUnix fork動作へ読み替えない。
            continue;
        }
        executed += 1;
        strict += usize::from(coverage == "strict");
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for path in case["directories"].as_array().unwrap() {
            fs::create_dir_all(root.join(path.as_str().unwrap())).unwrap();
        }
        let mut before = BTreeMap::new();
        for (path, text) in case["files"].as_object().unwrap() {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, text.as_str().unwrap().as_bytes()).unwrap();
            let metadata = fs::metadata(target).unwrap();
            before.insert(
                path.clone(),
                (
                    metadata.modified().unwrap(),
                    metadata.permissions().readonly(),
                ),
            );
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(root).env_remove("NO_COLOR");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
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
        let output = child.wait_with_output().unwrap();
        let expected = &case["expected"];
        if output.status.code().map(i64::from) != expected["exit"].as_i64() {
            differences.push(format!("{id}: exit {:?}", output.status.code()));
        }
        let actual = template(&output.stdout, root);
        let expected_stdout = native(&expected["stdout_hex"]);
        if actual != expected_stdout {
            differences.push(format!("{id}: stdout {actual:?} != {expected_stdout:?}"));
        }
        let stderr = template(&output.stderr, root);
        let mut expected_stderr = native(&expected["stderr_hex"]);
        if case["coverage"] == "deferred_158" {
            let notice = native(&Value::String(
                "parallel: Process.fork is not supported by this Ruby\r\n"
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            ));
            let start = expected_stderr
                .windows(notice.len())
                .position(|part| part == notice)
                .unwrap();
            expected_stderr.drain(start..start + notice.len());
        }
        if expected["stderr_contract"] == "open-error" {
            // generic Ruby backtraceは対象外。openが設定訪問より先であることを検査する。
            if stderr.is_empty() || String::from_utf8_lossy(&stderr).contains("nested/.rubocop.yml")
            {
                differences.push(format!("{id}: output open did not precede config lookup"));
            }
        } else if stderr != expected_stderr {
            differences.push(format!("{id}: stderr {stderr:?} != {expected_stderr:?}"));
        }
        for (path, expected) in expected["outputs"].as_object().unwrap() {
            let target = root.join(path);
            if expected.is_null() {
                if target.exists() {
                    differences.push(format!("{id}: unexpected output {path}"));
                }
            } else if let Ok(content) = fs::read(target) {
                if template(&content, root) != native(expected) {
                    differences.push(format!("{id}: output {path} differs"));
                }
            } else {
                differences.push(format!("{id}: output {path} missing"));
            }
        }
        for (path, expected_bytes) in expected["files_after"].as_object().unwrap() {
            let mut expected_bytes = bytes(expected_bytes);
            let original = case["files"][path].as_str().unwrap().as_bytes();
            if !cfg!(windows) && expected_bytes != original {
                // 有限入力の変更対象はASCII代入だけ。DATA/literal/元CRLFの投影へ広げない。
                assert!(matches!(original, b"x=1\n" | b"z=3\n"));
                expected_bytes = String::from_utf8(expected_bytes)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .into_bytes();
            }
            if fs::read(root.join(path)).unwrap() != expected_bytes {
                differences.push(format!("{id}: source {path} differs"));
            }
            if fs::metadata(root.join(path))
                .unwrap()
                .permissions()
                .readonly()
                != before[path].1
            {
                differences.push(format!("{id}: source {path} readonly differs"));
            }
        }
        for path in expected["unchanged_files"].as_array().unwrap() {
            let path = path.as_str().unwrap();
            let metadata = fs::metadata(root.join(path)).unwrap();
            if (
                metadata.modified().unwrap(),
                metadata.permissions().readonly(),
            ) != before[path]
            {
                differences.push(format!("{id}: unchanged source metadata {path} differs"));
            }
        }
    }
    assert_eq!(executed, if cfg!(windows) { 38 } else { 34 });
    assert_eq!(strict, 33);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn displayed_offenses_control_exit_status_for_disk_and_both_stdin_channels() {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/late_config_display_filter.json")).unwrap();
    assert_eq!(data["schema"], 1);
    assert_eq!(data["positive_control"]["id"], "unfiltered-disk-control");
    assert_eq!(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "filtered-disk",
            "filtered-stdin-file",
            "filtered-stdin-directory"
        ]
    );
    for case in std::iter::once(&data["positive_control"]).chain(data["cases"].as_array().unwrap())
    {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut before = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = root.join(name);
            fs::write(&path, text.as_str().unwrap()).unwrap();
            before.push((
                path.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().modified().unwrap(),
                fs::metadata(&path).unwrap().permissions().readonly(),
            ));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(root).env_remove("NO_COLOR");
        for name in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(name);
        }
        for (name, value) in case["env"].as_object().unwrap() {
            command.env(name, value.as_str().unwrap());
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        );
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
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|value| value as i32),
            "{}",
            case["id"]
        );
        let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
        assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
        let mut expected: Value = serde_json::from_slice(&bytes(&case["stdout_hex"])).unwrap();
        actual.as_object_mut().unwrap().remove("metadata");
        expected.as_object_mut().unwrap().remove("metadata");
        assert_eq!(actual, expected, "{}", case["id"]);
        assert!(output.stderr.is_empty(), "{}", case["id"]);
        assert!(!root.join("report.json").exists());
        for (path, original, modified, readonly) in before {
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
            assert_eq!(
                fs::metadata(path).unwrap().permissions().readonly(),
                readonly
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn formatter_file_write_failure_does_not_retry_with_an_empty_finished_report() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("config.yml"), "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  DisabledByDefault: true\nLayout/SpaceAroundOperators:\n  Enabled: true\n").unwrap();
    fs::write(root.join("Gemfile"), "").unwrap();
    let original = "x=1\n".repeat(500);
    let path = root.join("a.rb");
    fs::write(&path, &original).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    // 実OSのwriteをfile_finished途中で失敗させ、後続JSONのfinishedが再生されないことを見る。
    let output = Command::new(env!("CARGO_BIN_EXE_sonicop"))
        .current_dir(root)
        .args([
            "--config",
            "config.yml",
            "--cache",
            "false",
            "--format",
            "emacs",
            "--out",
            "/dev/full",
            "--format",
            "json",
            "--out",
            "report.json",
            "a.rb",
        ])
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .env_remove("RUBOCOP_OPTS")
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "0")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to write /dev/full"));
    assert_eq!(fs::read(root.join("report.json")).unwrap(), b"");
    assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
}
