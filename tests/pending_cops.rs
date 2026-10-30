//! PendingCopsReporterの実36条件を、全stderrと報告・早期出力へ照合する。

use std::fs;
use std::process::Command;

use serde_json::Value;

fn report_bytes(raw: &[u8], parallel: bool) -> &[u8] {
    if parallel {
        let notice = if cfg!(windows) {
            b"-P/--parallel is being ignored because it is not compatible with --cache false.\r\n"
                .as_slice()
        } else {
            b"-P/--parallel is being ignored because it is not compatible with --cache false.\n"
                .as_slice()
        };
        raw.strip_prefix(notice)
            .expect("既存135通知が先頭に一度だけ必要")
    } else {
        raw
    }
}

fn bytes(value: &Value) -> Vec<u8> {
    let hex = value.as_str().unwrap();
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn native(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

#[test]
fn pending_notice_preserves_all_recorded_streams_and_early_boundaries() {
    let data: Value = serde_json::from_str(include_str!("fixtures/pending_cops.json")).unwrap();
    assert_eq!(data["schema"], 1);
    let cases = data["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 36);
    let mut counts = (0, 0, 0);
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let directory = tempfile::tempdir().unwrap();
        for name in case["directories"].as_array().unwrap() {
            fs::create_dir(directory.path().join(name.as_str().unwrap())).unwrap();
        }
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
            let metadata = fs::metadata(&path).unwrap();
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                metadata.modified().unwrap(),
                metadata.permissions(),
            ));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(directory.path())
            .env("PATH", "")
            .env("TERM", "dumb");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
            "CLICOLOR_FORCE",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        let output = command
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{id}"
        );
        let stderr = native(bytes(&case["stderr_hex"]));
        assert!(
            output.stderr == stderr,
            "{id}: stderr differs ({} vs {} bytes)",
            output.stderr.len(),
            stderr.len()
        );
        match case["comparison"].as_str().unwrap() {
            mode @ ("json" | "json_parallel") => {
                let expected = native(bytes(&case["stdout_hex"]));
                let mut reference: Value =
                    serde_json::from_slice(report_bytes(&expected, mode == "json_parallel"))
                        .unwrap();
                let mut actual: Value =
                    serde_json::from_slice(report_bytes(&output.stdout, mode == "json_parallel"))
                        .unwrap();
                assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
                assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
                actual.as_object_mut().unwrap().remove("metadata");
                reference.as_object_mut().unwrap().remove("metadata");
                assert_eq!(actual, reference, "{id}");
                counts.0 += 1;
            }
            "progress" | "raw_mode" => {
                let expected = native(bytes(&case["stdout_hex"]));
                assert!(output.stdout == expected, "{id}: stdout differs");
                counts.1 += 1;
            }
            "version_identity" => {
                let expected = if id == "force-version" {
                    format!("{}\n", sonicop::VERSION)
                } else {
                    format!(
                        "sonicop {} (RuboCop {} CLI, tree-sitter-ruby v26.10.105, analyzing as Ruby 2.7) [{} {}]\n",
                        sonicop::VERSION,
                        sonicop::RUBOCOP_COMPAT_VERSION,
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    )
                };
                let expected = if cfg!(windows) {
                    expected.replace('\n', "\r\n")
                } else {
                    expected
                };
                assert_eq!(output.stdout, expected.as_bytes(), "{id}");
                counts.2 += 1;
            }
            mode => panic!("unknown comparison {mode}"),
        }
        for (path, original, modified, permissions) in originals {
            let metadata = fs::metadata(&path).unwrap();
            assert_eq!(fs::read(&path).unwrap(), original, "{id}");
            assert_eq!(metadata.modified().unwrap(), modified, "{id}");
            assert_eq!(metadata.permissions(), permissions, "{id}");
        }
        assert!(
            !directory.path().join("report.json").try_exists().unwrap(),
            "{id}"
        );
    }
    assert_eq!(counts, (29, 5, 2));
}
