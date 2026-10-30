//! 明示したWindowsのprefixを、通常origin・継承・拒否・実Ruby登録と並べて照合する。

#![cfg(windows)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;

fn fixture() -> Value {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/verbatim_config_origins.json")).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["cases"].as_array().unwrap().len(), 10);
    value
}

fn bind(text: &str, cwd: &std::path::Path) -> String {
    text.replace("<CWD_FWD>", &cwd.to_string_lossy().replace('\\', "/"))
        .replace("<CWD>", &cwd.to_string_lossy())
}

fn bytes(value: &Value) -> Vec<u8> {
    let (pairs, rest) = value.as_str().unwrap().as_bytes().as_chunks::<2>();
    assert!(rest.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn ruby() -> Option<&'static PathBuf> {
    static RUBY: OnceLock<Option<PathBuf>> = OnceLock::new();
    RUBY.get_or_init(|| {
        let executable = std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into());
        let mut command = Command::new(executable);
        for key in ["RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION", "NO_COLOR"] {
            command.env_remove(key);
        }
        let done = command.args(["-rrbconfig", "-e", "gem 'rubocop', '1.89.0'; require 'rubocop'; abort 'wrong Ruby' unless RUBY_VERSION == '4.0.7'; print RbConfig.ruby"]).output().ok()?;
        done.status.success().then(|| PathBuf::from(String::from_utf8(done.stdout).unwrap()))
    }).as_ref()
}

fn inspect(row: &Value, fixture: &Value, ruby: Option<&PathBuf>) {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let mut originals = Vec::new();
    for (name, text) in row["files"].as_object().unwrap() {
        let path = cwd.join(name);
        let data = bind(text.as_str().unwrap(), cwd).into_bytes();
        fs::write(&path, &data).unwrap();
        let info = fs::metadata(&path).unwrap();
        originals.push((path, data, info.modified().unwrap(), info.permissions()));
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(cwd)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CACHE_HOME", cwd.join("xdg-cache"))
        .env("XDG_CONFIG_HOME", cwd.join("empty-config"));
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBOCOP_CACHE_ROOT",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
    ] {
        command.env_remove(key);
    }
    if let Some(ruby) = ruby {
        command.env("SONICOP_RUNTIME_RUBY", ruby);
    }
    for arg in row["args"].as_array().unwrap() {
        command.arg(bind(arg.as_str().unwrap(), cwd));
    }
    let output = command.output().unwrap();
    let id = row["id"].as_str().unwrap();
    assert_eq!(
        output.status.code().map(i64::from),
        row["exit"].as_i64(),
        "{id}: {output:?}"
    );
    assert_eq!(
        output.stderr,
        bind(row["stderr_template"].as_str().unwrap(), cwd).as_bytes(),
        "{id}"
    );
    let reference = bytes(&row["stdout_hex"]);
    if reference.is_empty() {
        assert!(output.stdout.is_empty(), "{id}");
    } else {
        let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut reference: Value = serde_json::from_slice(&reference).unwrap();
        assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
        assert_eq!(reference["metadata"]["ruby_engine"], "ruby");
        for key in fixture["metadata_identity_keys"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            actual["metadata"].as_object_mut().unwrap().remove(key);
            reference["metadata"].as_object_mut().unwrap().remove(key);
        }
        assert_eq!(actual, reference, "{id}");
    }
    for (path, data, mtime, permissions) in originals {
        assert_eq!(fs::read(&path).unwrap(), data, "{id}");
        let after = fs::metadata(path).unwrap();
        assert_eq!(after.modified().unwrap(), mtime, "{id}");
        assert_eq!(after.permissions(), permissions, "{id}");
    }
    let marker = cwd.join("loaded_marker");
    if row["marker_hex"].is_null() {
        assert!(!marker.exists(), "{id}");
    } else {
        assert_eq!(fs::read(marker).unwrap(), bytes(&row["marker_hex"]), "{id}");
    }
}

#[test]
fn eight_ruby_free_origins_match_complete_reference_streams() {
    let fixture = fixture();
    let rows: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["runtime_required"] == false)
        .collect();
    assert_eq!(rows.len(), 8);
    for row in rows {
        inspect(row, &fixture, None);
    }
}

#[test]
fn two_real_ruby_origins_keep_the_prefix_during_registration() {
    let Some(ruby) = ruby() else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "実Ruby必須gateに固定Ruby/RuboCopがありません"
        );
        eprintln!("未計測: 実Rubyが無いため登録二条件を互換成功へ数えません");
        return;
    };
    let fixture = fixture();
    let rows: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["runtime_required"] == true)
        .collect();
    assert_eq!(rows.len(), 2);
    for row in rows {
        inspect(row, &fixture, Some(ruby));
    }
}
