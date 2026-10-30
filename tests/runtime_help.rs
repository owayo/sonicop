//! help前の実require、早期終了とat_exitを、実本家の参照から検査する。

use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/runtime_help.json")).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    let raw = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect::<Vec<_>>();
    if cfg!(windows) {
        raw
    } else {
        // 参照の採取OSはWindows。ASCII featureと先頭行の改行だけを投影する。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn environment(command: &mut Command, row: &Value) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
        "RUBOCOP_CACHE_ROOT",
    ] {
        command.env_remove(key);
    }
    command.env("TERM", "dumb").env("CLICOLOR_FORCE", "0");
    for (name, value) in row["environment"].as_object().unwrap() {
        command.env(name, value.as_str().unwrap());
    }
}

fn actual_ruby() -> Option<PathBuf> {
    let ruby = std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into());
    let mut command = Command::new(ruby);
    environment(&mut command, &serde_json::json!({"environment":{}}));
    let output = command
        .args([
            "-rrbconfig",
            "-e",
            "gem 'rubocop', '1.89.0'; require 'rubocop'; print RbConfig.ruby",
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8(output.stdout).unwrap()))
}

fn check(wrapper: bool) {
    let Some(ruby) = actual_ruby() else {
        assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
        eprintln!("未計測: help境界二十五条件の実Rubyがありません");
        return;
    };
    let data = fixture();
    let rows = data["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 25);
    let mut preserved = 0;
    for row in rows {
        let directory = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for (name, text) in row["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::write(&path, text.as_str().unwrap()).unwrap();
            let info = fs::metadata(&path).unwrap();
            originals.push((
                path.clone(),
                fs::read(path).unwrap(),
                info.modified().unwrap(),
                info.permissions(),
            ));
        }
        let mut command = if wrapper {
            let mut command = Command::new(&ruby);
            command
                .arg("-I")
                .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib"))
                .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("exe/sonicop"))
                .env("SONICOP_BINARY", env!("CARGO_BIN_EXE_sonicop"));
            command
        } else {
            Command::new(env!("CARGO_BIN_EXE_sonicop"))
        };
        environment(&mut command, row);
        command
            .current_dir(directory.path())
            .env("SONICOP_RUNTIME_RUBY", &ruby)
            .args(
                row["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap()),
            );
        let actual = command.output().unwrap();
        let id = row["id"].as_str().unwrap();
        assert_eq!(
            actual.status.code().map(i64::from),
            row["exit"].as_i64(),
            "{id}: {actual:?}"
        );
        assert_eq!(actual.stderr, bytes(&row["stderr_native_hex"]), "{id}");
        let mut prefix = bytes(&row["stdout_prefix_hex"]);
        let suffix = bytes(&row["stdout_suffix_hex"]);
        if id == "version-control" {
            let newline = if cfg!(windows) { "\r\n" } else { "\n" };
            let value = String::from_utf8(prefix).unwrap();
            assert!(value.ends_with(&format!("1.89.0{newline}")));
            prefix = value
                .replace(
                    &format!("1.89.0{newline}"),
                    &format!("{}{newline}", env!("CARGO_PKG_VERSION")),
                )
                .into_bytes();
        }
        if row["help"] == true {
            assert!(actual.stdout.starts_with(&prefix), "{id}: {actual:?}");
            assert!(actual.stdout.ends_with(&suffix), "{id}: {actual:?}");
            assert!(actual.stdout.len() >= prefix.len() + suffix.len(), "{id}");
            let help = std::str::from_utf8(
                &actual.stdout[prefix.len()..actual.stdout.len() - suffix.len()],
            )
            .unwrap();
            // 製品ごとのhelp本文は一致と呼ばず、featureとhelpの境界を検査する。
            assert!(help.contains("Usage: sonicop"), "{id}: {help}");
            assert!(
                !help.contains("_MARKER") && !help.contains("FINAL_LOAD"),
                "{id}"
            );
        } else {
            prefix.extend(suffix);
            assert_eq!(actual.stdout, prefix, "{id}");
        }
        for (path, body, modified, permissions) in originals {
            assert_eq!(fs::read(&path).unwrap(), body, "{id}");
            let info = fs::metadata(path).unwrap();
            assert_eq!(info.modified().unwrap(), modified, "{id}");
            assert_eq!(info.permissions(), permissions, "{id}");
            preserved += 1;
        }
    }
    assert_eq!(
        preserved,
        rows.iter()
            .map(|row| row["files"].as_object().unwrap().len())
            .sum::<usize>()
    );
}

#[test]
fn native_help_runs_only_prior_features_and_finishes_the_runtime() {
    check(false);
}

#[test]
fn wrapper_help_runs_only_prior_features_and_finishes_the_runtime() {
    check(true);
}

#[test]
fn help_before_features_keeps_the_ruby_free_path() {
    for args in [
        ["--help", "-r", "./absent.rb"],
        ["-h", "--plugin", "./absent.rb"],
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        environment(&mut command, &serde_json::json!({"environment":{}}));
        let actual = command
            .args(args)
            .env("PATH", "")
            .env("SONICOP_RUNTIME_RUBY", "absent-ruby")
            .output()
            .unwrap();
        assert!(actual.status.success(), "{actual:?}");
        assert!(actual.stderr.is_empty());
        assert!(
            String::from_utf8(actual.stdout)
                .unwrap()
                .contains("Usage: sonicop")
        );
    }
}
