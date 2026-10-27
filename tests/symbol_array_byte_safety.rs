//! 不正Symbolのlossy予定補正だけを既存のファイル単位拒否へ結線する。

use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/symbol_array_byte_safety.json")).unwrap();
    assert_eq!(fixture["issue"], 174);
    assert!(fixture["original_VM"]["value"].is_null());
    for mode in ["inspect", "-a", "-A"] {
        let record = &fixture["reference_failures"][mode];
        assert_eq!(record["exit"], 2);
        assert_eq!(record["inspection_complete"], false);
        assert_eq!(record["report"]["summary"]["inspected_file_count"], 0);
        assert_eq!(record["after_hex"], fixture["source_hex"]);
        assert!(record["report"]["files"].as_array().unwrap().is_empty());
    }
    let reverse = &fixture["reverse"];
    assert_eq!(
        reverse["reference_modes_verified"],
        serde_json::json!(["-A"])
    );
    assert_eq!(
        reverse["unmeasured_reference_modes"],
        serde_json::json!(["inspect", "-a"])
    );
    assert_eq!(reverse["original_VM"]["error_class"], "SyntaxError");
    assert_eq!(reverse["reference_failure"]["inspection_complete"], false);
    assert_eq!(
        reverse["reference_failure"]["after_hex"],
        reverse["source_hex"]
    );
    assert_eq!(
        reverse["reference_failure"]["report"]["summary"]["inspected_file_count"],
        0
    );
    let reverse_raw: Value =
        serde_json::from_slice(&bytes(&reverse["reference_failure"]["stdout_hex"])).unwrap();
    assert_eq!(reverse_raw, reverse["reference_failure"]["report"]);
    for control in fixture["controls"].as_array().unwrap() {
        for record in control["records"].as_object().unwrap().values() {
            let raw: Value = serde_json::from_slice(&bytes(&record["stdout_hex"])).unwrap();
            assert_eq!(raw, record["report"]);
            assert_eq!(record["original_values"], record["after_values"]);
        }
    }
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

fn command(directory: &Path, cache: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory).args([
        "--config",
        "config.yml",
        "--cache",
        if cache { "true" } else { "false" },
        "--only",
        "Style/SymbolArray,Layout/SpaceAroundOperators",
    ]);
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(key);
    }
    command
}

fn report(output: &Output) -> Value {
    let mut value: Value = serde_json::from_slice(&output.stdout).unwrap();
    value.as_object_mut().unwrap().remove("metadata");
    value
}

fn assert_rejected(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(
        stderr.contains("Style/SymbolArray would change the bytes of a symbol literal"),
        "{stderr}"
    );
    assert!(!stderr.contains("introduced a syntax error"), "{stderr}");
}

#[test]
fn symbol_array_invalid_decode_keeps_the_file_report_and_original_stdin() {
    let fixture = fixture();
    // 逆方向の本家実測は-A/LFのみ。-a/CRLFは候補の共通rollback方針を検査する。
    for (case, modes) in [
        (&fixture, [("\n", "-a"), ("\r\n", "-A")]),
        (&fixture["reverse"], [("\n", "-A"), ("\r\n", "-a")]),
    ] {
        let original = bytes(&case["source_hex"]);
        for (ending, mode) in modes {
            let directory = tempfile::tempdir().unwrap();
            fs::write(
                directory.path().join("config.yml"),
                case["config"].as_str().unwrap(),
            )
            .unwrap();
            let source = format!("{}x=1\n", String::from_utf8(original.clone()).unwrap())
                .replace('\n', ending)
                .into_bytes();
            let path = directory.path().join("input.rb");
            fs::write(&path, &source).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            let before = command(directory.path(), false)
                .args(["--format", "json", "input.rb"])
                .output()
                .unwrap();
            assert_eq!(before.status.code(), Some(1));
            let after = command(directory.path(), false)
                .args(["--format", "json", mode, "input.rb"])
                .output()
                .unwrap();
            assert_rejected(&after);
            assert_eq!(fs::read(&path).unwrap(), source);
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
            // 本家は正常な診断を返さないため、これはP期待でなくrollbackのS不変条件。
            assert_eq!(report(&before), report(&after));
            assert!(
                report(&after)["files"][0]["offenses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|o| o["corrected"] == false)
            );
        }
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
        let mut child = command(directory.path(), false)
            .args([
                "--stderr", "--format", "simple", "-A", "--stdin", "input.rb",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&original).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_rejected(&output);
        assert_eq!(output.stdout, original);
        assert!(!directory.path().join("input.rb").exists());
    }
    let original = bytes(&fixture["source_hex"]);
    // 設定とdirectiveは既存correctorの採用条件へ委ね、拒否を追加しない。
    for (setting, prefix, mode) in [
        ("  AutoCorrect: false\n", "", "-A"),
        ("  SafeAutoCorrect: false\n", "", "-a"),
        ("", "# rubocop:disable Style/SymbolArray\n", "-A"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("config.yml"),
            format!("{}{setting}", fixture["config"].as_str().unwrap()),
        )
        .unwrap();
        fs::write(
            directory.path().join("input.rb"),
            format!(
                "{prefix}{}x=1\n",
                String::from_utf8(original.clone()).unwrap()
            ),
        )
        .unwrap();
        let output = command(directory.path(), false)
            .args(["--format", "json", mode, "input.rb"])
            .output()
            .unwrap();
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "{setting:?}/{prefix:?}/{mode}: {:?}",
            output.status.code()
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("Autocorrection was not written")
        );
        let after = fs::read_to_string(directory.path().join("input.rb")).unwrap();
        assert!(after.contains("%I[\\x95 foo bar]"));
        assert!(after.contains("x = 1"));
        assert!(!after.contains('\u{fffd}'));
    }
    // 診断cacheに拒否markerを永続化せず、補正時の再検査で同じ保護を作る。
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("config.yml"),
        fixture["config"].as_str().unwrap(),
    )
    .unwrap();
    fs::write(directory.path().join("input.rb"), &original).unwrap();
    let before = command(directory.path(), true)
        .args(["--cache-root", "cache", "--format", "json", "input.rb"])
        .output()
        .unwrap();
    assert_eq!(before.status.code(), Some(1));
    let after = command(directory.path(), true)
        .args([
            "--cache-root",
            "cache",
            "--format",
            "json",
            "-a",
            "input.rb",
        ])
        .output()
        .unwrap();
    assert_rejected(&after);
    assert_eq!(
        fs::read(directory.path().join("input.rb")).unwrap(),
        original
    );
    assert_eq!(report(&before), report(&after));
}

#[test]
fn symbol_array_valid_controls_match_recorded_reference_reports_and_bytes() {
    let fixture = fixture();
    for control in fixture["controls"].as_array().unwrap() {
        for mode in ["inspect", "-a", "-A"] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(
                directory.path().join("config.yml"),
                fixture["config"].as_str().unwrap(),
            )
            .unwrap();
            fs::write(
                directory.path().join("input.rb"),
                bytes(&control["source_hex"]),
            )
            .unwrap();
            let mut command = command(directory.path(), false);
            command.args(["--format", "json"]);
            if mode != "inspect" {
                command.arg(mode);
            }
            let output = command.arg("input.rb").output().unwrap();
            let expected = &control["records"][mode];
            assert_eq!(
                output.status.code().map(i64::from),
                expected["exit"].as_i64(),
                "{}/{mode}",
                control["id"]
            );
            assert_eq!(output.stderr, bytes(&expected["stderr_hex"]));
            let mut expected_report = expected["report"].clone();
            expected_report.as_object_mut().unwrap().remove("metadata");
            assert_eq!(report(&output), expected_report, "{}/{mode}", control["id"]);
            let mut after = bytes(&expected["after_hex"]);
            // 保存本家の一行ASCIIコードの物理CRLFだけをhostへ合わせ、literalのbytesは投影しない。
            if !cfg!(windows) {
                after = String::from_utf8(after)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .into_bytes();
            }
            assert_eq!(
                fs::read(directory.path().join("input.rb")).unwrap(),
                after,
                "{}/{mode}",
                control["id"]
            );
        }
    }
}
