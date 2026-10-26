//! WordArrayのbyte値を失う予定補正だけを、ファイル単位で原本へ戻す。

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::Value;

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/word_array_byte_safety.json")).unwrap();
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let entry = &manifest["word_array_byte_safety_divergence"];
    assert_eq!(entry["issue"].as_u64(), Some(172));
    assert_eq!(entry["candidate_exit"].as_u64(), Some(2));
    assert_eq!(
        entry["reference_fixture"].as_str(),
        Some("tests/fixtures/word_array_byte_safety.json")
    );
    assert_eq!(fixture["controls"].as_array().unwrap().len(), 3);
    assert_eq!(fixture["reference_api"]["parser_version"], "3.3.12.0");
    assert_eq!(fixture["reference_api"]["display_cop_names"], false);
    assert_eq!(
        fixture["reference_api"]["offenses"][0]["message_record"]["valid_encoding"],
        false
    );
    assert_eq!(fixture["reference_json_failure"]["-a"]["exit"], 2);
    assert_eq!(
        fixture["reference_json_failure"]["-A"]["standard_JSON_succeeded"],
        false
    );
    // 既存候補のU+FFFDを安全側の期待値へ移さない。
    assert_eq!(
        fixture["expected_safe_policy"]["output_hex"],
        fixture["source_hex"]
    );
    assert_ne!(
        fixture["source_hex"],
        fixture["historical_lossy_native_not_an_expectation"]["-A"]["output_hex"]
    );
    fixture
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert!(text.len().is_multiple_of(2));
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn command(directory: &Path, cache: bool, only: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory).args([
        "--config",
        "config.yml",
        "--cache",
        if cache { "true" } else { "false" },
        "--only",
        only,
    ]);
    for name in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(name);
    }
    if cache {
        command.args(["--cache-root", "cache"]);
    }
    command
}

fn report(output: &Output) -> Value {
    let mut report: Value = serde_json::from_slice(&output.stdout).unwrap();
    report.as_object_mut().unwrap().remove("metadata");
    report
}

fn assert_rejection(output: &Output, fixture: &Value) {
    assert_eq!(output.status.code(), Some(2));
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("Autocorrection was not written to "),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            fixture["expected_safe_policy"]["reason_contains"]
                .as_str()
                .unwrap()
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("introduced a syntax error"), "{stderr}");
}

fn native_disk_bytes(original: &[u8], corrected: &[u8]) -> Vec<u8> {
    if cfg!(windows) && original != corrected && !original.contains(&b'\r') {
        assert!(!corrected.contains(&b'\r'));
        corrected
            .iter()
            .flat_map(|&byte| {
                if byte == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![byte]
                }
            })
            .collect()
    } else {
        corrected.to_vec()
    }
}

#[test]
fn word_array_file_rollback_preserves_original_bytes_reports_and_stdin() {
    let fixture = fixture();
    let mut original = bytes(&fixture["source_hex"]);
    original.extend_from_slice(b"x=1\n");
    let only = "Style/WordArray,Layout/SpaceAroundOperators";
    for crlf in [false, true] {
        let source = if crlf {
            String::from_utf8(original.clone())
                .unwrap()
                .replace('\n', "\r\n")
                .into_bytes()
        } else {
            original.clone()
        };
        for mode in ["-a", "-A"] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(
                directory.path().join("config.yml"),
                fixture["config"].as_str().unwrap(),
            )
            .unwrap();
            let path = directory.path().join("input.rb");
            fs::write(&path, &source).unwrap();
            let before = command(directory.path(), false, only)
                .args(["-f", "json", "input.rb"])
                .output()
                .unwrap();
            let before_report = report(&before);
            let actual_word = before_report["files"][0]["offenses"]
                .as_array()
                .unwrap()
                .iter()
                .find(|offense| offense["cop_name"] == "Style/WordArray")
                .unwrap();
            let mut actual_fields = actual_word.clone();
            actual_fields.as_object_mut().unwrap().remove("message");
            let mut reference_fields = fixture["reference_api"]["offenses"][0].clone();
            // 本家のmessageはUTF-8不正である。表示へ変換したものを本家のrawと呼ばない。
            reference_fields
                .as_object_mut()
                .unwrap()
                .remove("message_record");
            assert_eq!(actual_fields, reference_fields);
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            let corrected = command(directory.path(), false, only)
                .args(["-f", "json", mode, "input.rb"])
                .output()
                .unwrap();
            assert_rejection(&corrected, &fixture);
            assert_eq!(fs::read(&path).unwrap(), source);
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
            // 原本の再検査で、別copの既適用表示もすべて未補正へ戻る。
            assert_eq!(report(&corrected), before_report);
            assert!(
                report(&corrected)["files"][0]["offenses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|offense| offense["corrected"] == false)
            );

            let mut child = command(directory.path(), false, only)
                .args(["--stderr", "-f", "simple", mode, "--stdin", "input.rb"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&source).unwrap();
            let stdin = child.wait_with_output().unwrap();
            assert_rejection(&stdin, &fixture);
            // 拒否した原bytesへWindows text modeを後掛けしない。
            assert_eq!(stdin.stdout, source);
            assert_eq!(fs::read(&path).unwrap(), source);
        }
    }
}

#[test]
fn word_array_byte_guard_respects_controls_configuration_and_warm_diagnostic_cache() {
    let fixture = fixture();
    for control in fixture["controls"].as_array().unwrap() {
        for mode in ["-a", "-A"] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(
                directory.path().join("config.yml"),
                fixture["config"].as_str().unwrap(),
            )
            .unwrap();
            let source = bytes(&control["source_hex"]);
            fs::write(directory.path().join("input.rb"), &source).unwrap();
            let output = command(directory.path(), false, "Style/WordArray")
                .args(["-f", "json", mode, "input.rb"])
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                control["reference"][mode]["exit"]
                    .as_i64()
                    .map(|value| value as i32)
            );
            assert!(output.stderr.is_empty());
            let mut expected = bytes(&control["reference"][mode]["after_windows_hex"]);
            if !cfg!(windows) && expected != source {
                expected = String::from_utf8(expected)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .into_bytes();
            }
            assert_eq!(
                fs::read(directory.path().join("input.rb")).unwrap(),
                expected
            );
            assert_eq!(
                report(&output),
                control["reference"][mode]["report_formatter_segment"]
            );
        }
    }
    let mut original = bytes(&fixture["source_hex"]);
    original.extend_from_slice(b"x=1\n");
    let only = "Style/WordArray,Layout/SpaceAroundOperators";
    for (setting, prefix, mode, except, reject) in [
        ("  AutoCorrect: false\n", "", "-A", false, false),
        ("  SafeAutoCorrect: false\n", "", "-a", false, false),
        ("  SafeAutoCorrect: false\n", "", "-A", false, true),
        (
            "",
            "# rubocop:disable Style/WordArray\n",
            "-A",
            false,
            false,
        ),
        ("", "", "-a", true, false),
    ] {
        let label = format!("{setting:?}/{prefix:?}/{mode}/except={except}");
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            format!("{}{setting}", fixture["config"].as_str().unwrap()),
        )
        .unwrap();
        let source = [prefix.as_bytes(), &original].concat();
        fs::write(directory.path().join("input.rb"), &source).unwrap();
        let mut cmd = command(directory.path(), false, only);
        cmd.args(["-f", "json", mode]);
        if except {
            cmd.args(["--except", "Style/WordArray"]);
        }
        let output = cmd.arg("input.rb").output().unwrap();
        if reject {
            assert_rejection(&output, &fixture);
            assert_eq!(
                fs::read(directory.path().join("input.rb")).unwrap(),
                source,
                "{label}"
            );
        } else {
            // 未補正の違反が残れば通常終了1になる。値保護による拒否2とは区別する。
            assert!(
                matches!(output.status.code(), Some(0 | 1)),
                "{label}: {:?}",
                output.status.code()
            );
            assert!(output.stderr.is_empty(), "{label}");
            let expected = String::from_utf8(source.clone())
                .unwrap()
                .replace("x=1", "x = 1")
                .into_bytes();
            assert_eq!(
                fs::read(directory.path().join("input.rb")).unwrap(),
                native_disk_bytes(&source, &expected),
                "{label}"
            );
            let observed = report(&output);
            let offenses = observed["files"][0]["offenses"].as_array().unwrap();
            assert!(
                offenses.iter().any(
                    |offense| offense["cop_name"] == "Layout/SpaceAroundOperators"
                        && offense["corrected"] == true
                ),
                "{label}"
            );
            assert!(
                offenses
                    .iter()
                    .all(|offense| offense["cop_name"] != "Style/WordArray"
                        || offense["corrected"] == false),
                "{label}"
            );
        }
    }
    let cached_source = bytes(&fixture["source_hex"]);
    for mode in ["-a", "-A"] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            fixture["config"].as_str().unwrap(),
        )
        .unwrap();
        let path = directory.path().join("input.rb");
        fs::write(&path, &cached_source).unwrap();
        let before = command(directory.path(), true, only)
            .args(["-f", "json", "input.rb"])
            .output()
            .unwrap();
        assert_eq!(before.status.code(), Some(1));
        assert!(
            directory
                .path()
                .join("cache")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let corrected = command(directory.path(), true, only)
            .args(["-f", "json", mode, "input.rb"])
            .output()
            .unwrap();
        assert_rejection(&corrected, &fixture);
        assert_eq!(fs::read(&path).unwrap(), cached_source);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(report(&corrected), report(&before));
    }
}
