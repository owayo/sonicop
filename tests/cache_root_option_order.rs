//! cache-root callbackの順序・先行require・全診断と原本を実本家へ照合する。
#![cfg(windows)]

use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/cache_root_option_order.json")).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn environment(command: &mut Command) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
        "RUBOCOP_CACHE_ROOT",
        "SONICOP_PROFILE",
    ] {
        command.env_remove(key);
    }
    command.env("TERM", "dumb").env("CLICOLOR_FORCE", "0");
}

fn actual_ruby() -> Option<PathBuf> {
    let mut command =
        Command::new(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    environment(&mut command);
    let done = command
        .args([
            "-rrbconfig",
            "-e",
            "gem 'rubocop', '1.89.0'; require 'rubocop'; print RbConfig.ruby",
        ])
        .output()
        .ok()?;
    done.status
        .success()
        .then(|| PathBuf::from(String::from_utf8(done.stdout).unwrap()))
}

fn check(wrapper: bool) {
    let ruby = actual_ruby();
    if wrapper && ruby.is_none() {
        assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
        eprintln!("未計測: 固定Rubyがないためwrapperの42条件は互換成功へ数えません");
        return;
    }
    let data = fixture();
    let rows = data["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 42);
    let mut complete = 0;
    for row in rows {
        if row["requires_runtime"] == true && ruby.is_none() {
            assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
            eprintln!("未計測: {}の実requireがありません", row["id"]);
            continue;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let originals: Vec<_> = row["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, body)| {
                let path = root.join(name);
                let raw = body.as_str().unwrap().as_bytes().to_vec();
                fs::write(&path, &raw).unwrap();
                let info = fs::metadata(&path).unwrap();
                (
                    name.clone(),
                    path,
                    raw,
                    info.modified().unwrap(),
                    info.permissions(),
                )
            })
            .collect();
        let mut command = if wrapper {
            let mut command = Command::new(ruby.as_ref().unwrap());
            command
                .arg("-I")
                .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib"))
                .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("exe/sonicop"))
                .env("SONICOP_BINARY", env!("CARGO_BIN_EXE_sonicop"));
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_sonicop"))
        };
        environment(&mut command);
        command
            .current_dir(root)
            .env("XDG_CACHE_HOME", root.join("xdg"))
            .env("XDG_CONFIG_HOME", root.join("empty-config"));
        if wrapper || row["requires_runtime"] == true {
            command.env("SONICOP_RUNTIME_RUBY", ruby.as_ref().unwrap());
        } else {
            // 拒否やhelpより後のrequireは、Rubyのない環境でも実行してはいけない。
            command
                .env("PATH", "")
                .env("SONICOP_RUNTIME_RUBY", "missing-ruby");
        }
        for (key, value) in row["environment"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        command.args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap()),
        );
        let actual = command.output().unwrap();
        let id = row["id"].as_str().unwrap();
        assert_eq!(
            actual.status.code().map(i64::from),
            row["exit"].as_i64(),
            "{id}: {actual:?}"
        );
        assert_eq!(actual.stderr, bytes(&row["stderr_hex"]), "{id}");
        let reference = bytes(&row["stdout_hex"]);
        match row["stdout_kind"].as_str().unwrap() {
            "json" | "help" => {
                let prefix = bytes(&row["stdout_prefix_hex"]);
                let suffix = bytes(&row["stdout_suffix_hex"]);
                assert!(
                    actual.stdout.len() >= prefix.len() + suffix.len(),
                    "{id}: {actual:?}"
                );
                assert!(actual.stdout.starts_with(&prefix), "{id}: {actual:?}");
                assert!(actual.stdout.ends_with(&suffix), "{id}: {actual:?}");
                let body = &actual.stdout[prefix.len()..actual.stdout.len() - suffix.len()];
                let expected = &reference[prefix.len()..reference.len() - suffix.len()];
                if row["stdout_kind"] == "json" {
                    let mut report: Value = serde_json::from_slice(body).unwrap();
                    let mut reference: Value = serde_json::from_slice(expected).unwrap();
                    assert_eq!(
                        report["metadata"]["sonicop_version"],
                        env!("CARGO_PKG_VERSION")
                    );
                    assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
                    assert_eq!(report["metadata"]["ruby_engine"], "sonicop");
                    assert_eq!(report["metadata"]["ruby_version"], "n/a");
                    assert_eq!(report["metadata"]["ruby_platform"], std::env::consts::OS);
                    for key in data["metadata_identity_keys"].as_array().unwrap() {
                        let key = key.as_str().unwrap();
                        report["metadata"].as_object_mut().unwrap().remove(key);
                        reference["metadata"].as_object_mut().unwrap().remove(key);
                    }
                    assert_eq!(report, reference, "{id}");
                } else {
                    assert_eq!(expected, bytes(&data["reference_help_hex"]));
                    let help = std::str::from_utf8(body).unwrap();
                    // 製品ごとのhelp本文は一致へ数えず、featureとの境界だけを比べる。
                    assert!(
                        help.contains("Usage: sonicop") && !help.contains("EXIT:"),
                        "{id}"
                    );
                }
            }
            "version" => {
                assert_eq!(reference, b"1.89.0\r\n");
                assert_eq!(
                    actual.stdout,
                    format!("{}\r\n", env!("CARGO_PKG_VERSION")).as_bytes()
                );
            }
            "verbose" => {
                assert!(reference.starts_with(b"1.89.0 "));
                let text = std::str::from_utf8(&actual.stdout).unwrap();
                assert!(text.starts_with(&format!("sonicop {} ", env!("CARGO_PKG_VERSION"))));
                assert!(
                    text.contains("RuboCop 1.89 CLI")
                        && text.contains("tree-sitter-ruby v26.10.105")
                );
                assert!(text.ends_with("\r\n"));
            }
            "bytes" => assert_eq!(actual.stdout, reference, "{id}"),
            other => panic!("{other}"),
        }
        for (name, path, raw, modified, permissions) in originals {
            let after = fs::metadata(&path).unwrap();
            assert_eq!(after.permissions(), permissions, "{id}: {name}");
            if name == "a.rb" && !row["corrected_source_hex"].is_null() {
                assert_eq!(
                    fs::read(&path).unwrap(),
                    bytes(&row["corrected_source_hex"]),
                    "{id}"
                );
            } else {
                assert_eq!(fs::read(&path).unwrap(), raw, "{id}: {name}");
                assert_eq!(after.modified().unwrap(), modified, "{id}: {name}");
            }
        }
        let marker = root.join("loaded_marker");
        if row["marker_hex"].is_null() {
            assert!(!marker.exists(), "{id}");
        } else {
            assert_eq!(fs::read(marker).unwrap(), bytes(&row["marker_hex"]), "{id}");
        }
        for (name, expected) in row["cache_roots_present"].as_object().unwrap() {
            assert_eq!(
                root.join(name).is_dir(),
                expected.as_bool().unwrap(),
                "{id}: {name}"
            );
        }
        complete += 1;
    }
    assert_eq!(complete, if ruby.is_some() { 42 } else { 37 });
}

#[test]
fn native_matches_forty_two_actual_option_order_and_runtime_conditions() {
    check(false);
}

#[test]
fn wrapper_matches_forty_two_actual_option_order_and_runtime_conditions() {
    check(true);
}
