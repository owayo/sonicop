//! 空configのI/O先頭行を実本家へ合わせ、内部backtraceとの比較範囲を分ける。
#![cfg(windows)]

use std::{fs, process::Command};

use serde_json::Value;

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn empty_config_reaches_io_without_confusing_missing_or_omitted_arguments() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/empty_config_arguments.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(cases.len(), 20);
    let ids: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "empty_separate",
            "empty_equals",
            "empty_short",
            "empty_notice",
            "empty_cache_false",
            "empty_no_parallel",
            "empty_version",
            "empty_existing_out",
            "directory",
            "normal",
            "omitted",
            "enoent",
            "missing_long",
            "missing_short",
            "empty_help",
            "empty_invalid_cache",
            "empty_unknown_option",
            "empty_then_normal",
            "normal_then_empty",
            "environment_then_empty"
        ]
    );
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        20
    );
    let mut checked = 0;
    for case in cases {
        let kind = match (case["kind"].as_str().unwrap(), case["id"].as_str().unwrap()) {
            ("override", "empty_then_normal") => "success",
            ("override", _) => "io",
            (kind, _) => kind,
        };
        // help/他引数の本文は既存差として分離し、空値とlast winsは17条件へ含む。
        if !matches!(kind, "io" | "success" | "missing" | "missing_argument") {
            continue;
        }
        checked += 1;
        let folder = tempfile::tempdir().unwrap();
        for (path, source) in case["files"].as_object().unwrap() {
            fs::write(folder.path().join(path), source.as_str().unwrap()).unwrap();
        }
        fs::create_dir(folder.path().join("configdir")).unwrap();
        let before: Vec<_> = case["files"]
            .as_object()
            .unwrap()
            .keys()
            .map(|path| {
                fs::metadata(folder.path().join(path))
                    .unwrap()
                    .modified()
                    .unwrap()
            })
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(folder.path()).env("PATH", "");
        for name in [
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "RUBYOPT",
            "RUBYLIB",
        ] {
            command.env_remove(name);
        }
        for name in ["TERM", "CLICOLOR_FORCE"] {
            if let Some(value) = fixture["runtime"]["rainbow_environment"][name].as_str() {
                command.env(name, value);
            } else {
                command.env_remove(name);
            }
        }
        for (name, value) in case["environment"].as_object().unwrap() {
            command.env(name, value.as_str().unwrap());
        }
        let output = command
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
            output.status.code(),
            case["exit"].as_i64().map(|code| code as i32),
            "{}",
            case["id"]
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        if kind == "io" {
            // 元の本家記録の先頭行を使い、旧OS error envelopeから昇格する。
            let expected = case["stderr_first_line"]
                .as_str()
                .unwrap()
                .replace("{cwd}", &folder.path().to_string_lossy().replace('\\', "/"));
            assert_eq!(stderr, format!("{expected}\r\n"), "{}", case["id"]);
        } else if kind == "missing_argument" {
            // 本文はOptionParserとの既存差。真に値がない場合はI/Oへ進まない。
            assert!(
                stderr.contains("a value is required"),
                "{}: {stderr}",
                case["id"]
            );
        } else if kind == "missing" {
            let expected = case["stderr_first_line"]
                .as_str()
                .unwrap()
                .replace("{cwd}", &folder.path().to_str().unwrap().replace('\\', "/"));
            assert_eq!(stderr, format!("{expected}\r\n"));
        } else {
            assert!(stderr.is_empty(), "{}: {stderr}", case["id"]);
        }
        let expected_stdout = bytes(case["stdout_hex"].as_str().unwrap());
        assert_eq!(output.stdout, expected_stdout, "{}", case["id"]);
        for ((path, hex), modified) in case["after_hex"].as_object().unwrap().iter().zip(before) {
            assert_eq!(
                fs::read(folder.path().join(path)).unwrap(),
                bytes(hex.as_str().unwrap())
            );
            assert_eq!(
                fs::metadata(folder.path().join(path))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
        }
        let out = folder.path().join("out.json");
        if kind == "success" {
            let mut actual: Value = serde_json::from_slice(&fs::read(out).unwrap()).unwrap();
            let mut expected: Value =
                serde_json::from_slice(&bytes(case["out_hex"].as_str().unwrap())).unwrap();
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            actual.as_object_mut().unwrap().remove("metadata");
            expected.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, expected);
        } else if let Some(hex) = case["out_hex"].as_str() {
            assert_eq!(fs::read(out).unwrap(), bytes(hex));
        } else {
            assert!(!out.exists(), "{} created --out", case["id"]);
        }
    }
    assert_eq!(checked, 17);
}
