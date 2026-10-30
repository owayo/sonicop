//! 本家のConfigNotFoundError本文と、別I/Oエラー・原本保持を分けて検査する。

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

fn native(raw: &[u8]) -> Vec<u8> {
    if cfg!(windows) {
        raw.to_vec()
    } else {
        String::from_utf8(raw.to_vec())
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn expand(value: &str, cwd: &std::path::Path) -> String {
    value
        .replace("{cwd}", &cwd.to_str().unwrap().replace('\\', "/"))
        .replace("{verbatim}", &format!("\\\\?\\{}", cwd.display()))
}

#[test]
fn missing_config_errors_match_the_real_body_without_hiding_other_io_errors() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/missing_config_errors.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(cases.len(), 17);
    let ids: std::collections::HashSet<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 17);
    let order: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        [
            "relative",
            "absolute",
            "parent",
            "backslash",
            "unicode",
            "inherit",
            "nested_inherit",
            "directory",
            "empty",
            "normal",
            "version",
            "color_option",
            "color_environment",
            "existing_out",
            "nested_missing",
            "verbatim",
            "verbatim_inherit"
        ]
    );
    for (scope, count) in [("missing", 14), ("json", 1), ("directory_envelope", 2)] {
        assert_eq!(
            cases
                .iter()
                .filter(|case| case["coverage"] == scope)
                .count(),
            count
        );
    }
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["kind"] == "missing")
            .count(),
        14
    );
    let mut measured = 0;
    for case in cases {
        if !cfg!(windows)
            && matches!(
                case["id"].as_str().unwrap(),
                "backslash" | "verbatim" | "verbatim_inherit"
            )
        {
            continue;
        }
        measured += 1;
        let folder = tempfile::tempdir().unwrap();
        for (path, source) in case["files"].as_object().unwrap() {
            let path = folder.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source.as_str().unwrap().as_bytes()).unwrap();
        }
        for directory in case["directories"].as_array().unwrap() {
            fs::create_dir_all(folder.path().join(directory.as_str().unwrap())).unwrap();
        }
        // Windowsの8.3 cwdは保持する。Unixの子cwdだけはOSが返すphysical pathへ合わせる。
        #[cfg(windows)]
        let display_cwd = folder.path().to_owned();
        #[cfg(not(windows))]
        let display_cwd = fs::canonicalize(folder.path()).unwrap();
        let mtimes: Vec<_> = case["files"]
            .as_object()
            .unwrap()
            .keys()
            .map(|name| {
                (
                    name,
                    fs::metadata(folder.path().join(name))
                        .unwrap()
                        .modified()
                        .unwrap(),
                )
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
                    .map(|arg| expand(arg.as_str().unwrap(), &display_cwd)),
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|value| value as i32),
            "{}",
            case["id"]
        );
        if case["kind"] == "io" {
            assert_eq!(case["error_class"], "Errno::EISDIR");
            // 元の実APIが返したmessageを使い、一般I/Oの先頭行までを照合する。
            let expected = format!(
                "{}\r\n",
                expand(
                    case["error_message_template"].as_str().unwrap(),
                    &display_cwd,
                )
            );
            assert_eq!(output.stderr, native(expected.as_bytes()), "{}", case["id"]);
        } else {
            let expected = String::from_utf8(bytes(case["stderr_hex"].as_str().unwrap())).unwrap();
            assert_eq!(
                output.stderr,
                native(expand(&expected, &display_cwd).as_bytes()),
                "{}",
                case["id"]
            );
        }
        let reference_stdout = bytes(case["stdout_hex"].as_str().unwrap());
        assert_eq!(output.stdout, native(&reference_stdout), "{}", case["id"]);
        for (path, after) in case["after_hex"].as_object().unwrap() {
            assert_eq!(
                fs::read(folder.path().join(path)).unwrap(),
                bytes(after.as_str().unwrap()),
                "{} {path}",
                case["id"]
            );
        }
        for (name, modified) in mtimes {
            assert_eq!(
                fs::metadata(folder.path().join(name))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified,
                "{}: {name}",
                case["id"]
            );
        }
        let out = folder.path().join("out.json");
        if case["kind"] == "success" {
            let mut actual: Value = serde_json::from_slice(&fs::read(out).unwrap()).unwrap();
            let mut expected: Value =
                serde_json::from_slice(&bytes(case["out_hex"].as_str().unwrap())).unwrap();
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            actual.as_object_mut().unwrap().remove("metadata");
            expected.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, expected);
        } else if let Some(hex) = case["out_hex"].as_str() {
            assert_eq!(fs::read(out).unwrap(), bytes(hex));
        } else {
            assert!(!out.exists(), "{} created --out", case["id"]);
        }
    }
    assert_eq!(measured, if cfg!(windows) { 17 } else { 14 });
}

#[test]
fn early_explicit_load_observes_errors_without_resolving_the_target_version() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/missing_config_early_phases.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    let ids: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "version-valid",
            "version-invalid-target",
            "force-default-invalid-target",
            "version-parameter",
            "verbose-valid",
            "version-inherit-missing",
            "force-inherit-missing",
            "version-scalar"
        ]
    );
    for case in cases {
        let folder = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let display_cwd = folder.path().to_owned();
        #[cfg(not(windows))]
        let display_cwd = fs::canonicalize(folder.path()).unwrap();
        let mut originals = Vec::new();
        for (name, source) in fixture["files"].as_object().unwrap() {
            let path = folder.path().join(name);
            fs::write(&path, source.as_str().unwrap()).unwrap();
            originals.push((
                path.clone(),
                fs::metadata(path).unwrap().modified().unwrap(),
            ));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(folder.path())
            .env("CLICOLOR_FORCE", "0")
            .env("TERM", "dumb");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
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
        let id = case["id"].as_str().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{id}: {output:?}"
        );
        match case["coverage"].as_str().unwrap() {
            "native_version" => {
                let stdout = String::from_utf8(output.stdout).unwrap();
                if id == "verbose-valid" {
                    // nativeの版identityは既存仕様。本家から採った実効TargetRubyだけを照合する。
                    assert!(stdout.starts_with(&format!("sonicop {} ", env!("CARGO_PKG_VERSION"))));
                    assert!(stdout.contains("analyzing as Ruby 2.7"));
                } else {
                    assert_eq!(
                        stdout.as_bytes(),
                        native(format!("{}\r\n", env!("CARGO_PKG_VERSION")).as_bytes())
                    );
                }
                assert_eq!(
                    output.stderr,
                    native(&bytes(case["stderr_hex"].as_str().unwrap())),
                    "{id}"
                );
            }
            "missing" => {
                assert!(output.stdout.is_empty(), "{id}");
                let expected =
                    String::from_utf8(bytes(case["stderr_hex"].as_str().unwrap())).unwrap();
                assert_eq!(
                    output.stderr,
                    native(expand(&expected, &display_cwd).as_bytes()),
                    "{id}"
                );
            }
            "json_pending_residual" => {
                let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
                let mut expected: Value =
                    serde_json::from_slice(&bytes(case["stdout_hex"].as_str().unwrap())).unwrap();
                assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
                assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
                actual.as_object_mut().unwrap().remove("metadata");
                expected.as_object_mut().unwrap().remove("metadata");
                assert_eq!(actual, expected, "{id}");
                // pending通知#157の未一致はこのTargetRuby負対照の成功へ数えない。
            }
            "malformed_envelope" => {
                assert!(output.stdout.is_empty());
                let stderr = String::from_utf8(output.stderr).unwrap();
                assert!(stderr.starts_with("Error: "), "{id}: {stderr}");
                assert!(stderr.contains("Malformed configuration"), "{id}: {stderr}");
                assert!(!stderr.contains("Configuration file not found:"));
            }
            _ => panic!("未知の比較scope"),
        }
        for (path, modified) in originals {
            let name = path.file_name().unwrap().to_str().unwrap();
            assert_eq!(
                fs::read(&path).unwrap(),
                fixture["files"][name].as_str().unwrap().as_bytes(),
                "{id}: {name}"
            );
            assert_eq!(
                fs::metadata(&path).unwrap().modified().unwrap(),
                modified,
                "{id}: {name}"
            );
        }
    }
}
