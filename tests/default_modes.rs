//! 既定copモードの真偽と拒否段階を、実本家のCLI・設定APIと比較する。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use serde_json::Value;
use sonicop::config::Config;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop'; require 'json';";

fn cases() -> Vec<Value> {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/default_mode_cases.json")).unwrap();
    let rows = data["cases"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 58);
    rows
}

fn environment(command: &mut Command, root: &Path, row: &Value) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBOCOP_CACHE_ROOT",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
    ] {
        command.env_remove(key);
    }
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CACHE_HOME", root.join("xdg"))
        .env("XDG_CONFIG_HOME", root.join("empty-config"));
    if let Some(values) = row["environment"].as_object() {
        for (name, value) in values {
            command.env(name, value.as_str().unwrap());
        }
    }
}

fn ruby(root: &Path) -> Option<PathBuf> {
    let ruby =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&ruby);
    environment(&mut command, root, &serde_json::json!({}));
    let result = command.args(["-e", LOADER]).output();
    if result.as_ref().is_ok_and(|done| done.status.success()) {
        Some(ruby)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定Rubyが必要です: {result:?}"
        );
        eprintln!("未計測: 実Ruby/RuboCopがないため既定モード比較は成功へ数えません");
        None
    }
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    bytes: Vec<u8>,
    modified: SystemTime,
    mode: u32,
}

fn snapshot(path: &Path) -> Snapshot {
    let info = fs::metadata(path).unwrap();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode()
    };
    #[cfg(windows)]
    let mode = u32::from(info.permissions().readonly());
    Snapshot {
        bytes: fs::read(path).unwrap(),
        modified: info.modified().unwrap(),
        mode,
    }
}

fn prepare(root: &Path, row: &Value) -> BTreeMap<String, Snapshot> {
    fs::create_dir(root).unwrap();
    row["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, body)| {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body.as_str().unwrap()).unwrap();
            (name.clone(), snapshot(&path))
        })
        .collect()
}

fn kept(root: &Path, before: BTreeMap<String, Snapshot>, row: &Value) {
    for (name, original) in before {
        assert_eq!(
            snapshot(&root.join(&name)),
            original,
            "{}: 原本{name}",
            row["id"]
        );
    }
}

fn normalize_root(text: String, root: &Path) -> String {
    text.replace(&root.to_string_lossy().replace('\\', "/"), "<ROOT>")
}

#[derive(Debug, PartialEq)]
struct Capture {
    stdout: Vec<u8>,
    stderr: String,
    value: Option<Value>,
    marker: Option<Vec<u8>>,
}

fn capture(root: &Path, row: &Value, reference: bool, ruby: &Path) -> Capture {
    let before = prepare(root, row);
    let mut command = if reference {
        let mut command = Command::new(ruby);
        command.args([
            "-e",
            &format!("{LOADER}\nexit RuboCop::CLI.new.run(ARGV)\n"),
            "--",
        ]);
        command
    } else {
        Command::new(env!("CARGO_BIN_EXE_sonicop"))
    };
    environment(&mut command, root, row);
    if !reference {
        if row["files"].get("feature.rb").is_some() {
            command.env("SONICOP_RUNTIME_RUBY", ruby);
        } else {
            command.env("PATH", "");
        }
    }
    let done = command
        .args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        )
        .output()
        .unwrap();
    assert_eq!(
        done.status.code().map(i64::from),
        row["status"].as_i64(),
        "{} / {reference}: {done:?}",
        row["id"]
    );
    kept(root, before, row);
    let mode_error = String::from_utf8_lossy(&done.stderr)
        .contains("Cops cannot be both enabled by default and disabled by default");
    assert_eq!(
        mode_error,
        row["mode_error"].as_bool().unwrap(),
        "{} / {reference}: {done:?}",
        row["id"]
    );
    let mut stdout = done.stdout;
    let scope = row["scope"].as_str().unwrap();
    let mut value = None;
    if scope == "enabled" {
        let mapping: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&stdout).unwrap();
        let enabled = mapping
            .get(row["cop"].as_str().unwrap())
            .unwrap()
            .get("Enabled")
            .unwrap();
        // 表示全体の互換性へ広げず、実測したEnabledの値だけを比べる。
        value = Some(serde_json::to_value(enabled).unwrap());
        stdout.clear();
    } else if scope == "version" {
        let text = String::from_utf8(stdout).unwrap();
        let version = if reference {
            "1.89.0"
        } else {
            env!("CARGO_PKG_VERSION")
        };
        assert_eq!(
            text,
            format!("{version}{}", if cfg!(windows) { "\r\n" } else { "\n" })
        );
        stdout = Vec::new();
    } else if scope == "verbose" {
        let text = String::from_utf8(stdout).unwrap();
        let identity = if reference {
            "1.89.0".to_owned()
        } else {
            format!("RuboCop {} CLI", sonicop::RUBOCOP_COMPAT_VERSION)
        };
        let target = row["target_display"].as_str().unwrap_or("2.7");
        assert!(
            text.contains(&identity) && text.contains(&format!("analyzing as Ruby {target}")),
            "{text}"
        );
        // 製品名・実装・parserの説明は別物。省略した検証をエラーなく通れることを検査する。
        stdout = Vec::new();
    } else if let Some(start) = stdout.iter().position(|byte| *byte == b'{') {
        assert_eq!(stdout.last(), Some(&b'}'));
        let mut report: Value = serde_json::from_slice(&stdout[start..]).unwrap();
        assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
        for key in [
            "sonicop_version",
            "ruby_engine",
            "ruby_version",
            "ruby_platform",
        ] {
            report["metadata"].as_object_mut().unwrap().remove(key);
        }
        value = Some(report);
        stdout.truncate(start);
    }
    let marker = fs::read(root.join("loaded_marker")).ok();
    if row["files"].get("feature.rb").is_some() {
        assert_eq!(
            marker.as_deref(),
            Some(b"loaded\n".as_slice()),
            "{} / {reference}",
            row["id"]
        );
        let ending = if cfg!(windows) { "\r\n" } else { "\n" };
        assert!(
            String::from_utf8_lossy(&done.stderr)
                .ends_with(&format!("leave:{}{ending}", row["status"])),
            "{} / {reference}",
            row["id"]
        );
    }
    Capture {
        stdout,
        stderr: normalize_root(String::from_utf8(done.stderr).unwrap(), root),
        value,
        marker,
    }
}

#[test]
fn default_modes_match_real_cli_values_and_failure_phases() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(not(windows))]
    let base = directory.path().canonicalize().unwrap();
    #[cfg(windows)]
    let base = directory.path().to_path_buf();
    let Some(ruby) = ruby(&base) else {
        return;
    };
    for row in cases() {
        let root = base.join(row["id"].as_str().unwrap());
        fs::create_dir(&root).unwrap();
        let reference = capture(&root.join("reference"), &row, true, &ruby);
        assert_eq!(
            capture(&root.join("native"), &row, false, &ruby),
            reference,
            "{}",
            row["id"]
        );
    }
}

#[test]
fn quiet_api_keeps_parent_fatals_and_ruby_mode_values() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(not(windows))]
    let base = directory.path().canonicalize().unwrap();
    #[cfg(windows)]
    let base = directory.path().to_path_buf();
    let Some(ruby) = ruby(&base) else {
        return;
    };
    let rows = cases()
        .into_iter()
        .filter(|row| {
            row["id"]
                .as_str()
                .unwrap()
                .starts_with("DisabledByDefault-")
                || row["id"].as_str().unwrap().starts_with("EnabledByDefault-")
                || [
                    "both-true",
                    "parent-conflict-child-overrides",
                    "parent-before-later-io",
                ]
                .contains(&row["id"].as_str().unwrap())
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 17);
    for row in rows {
        let root = base.join(row["id"].as_str().unwrap());
        let before = prepare(&root, &row);
        let config_name = if row["files"].get("config.yml").is_some() {
            "config.yml"
        } else {
            ".rubocop.yml"
        };
        let cop = row["cop"].as_str().unwrap_or("Layout/SpaceAroundOperators");
        let mut command = Command::new(&ruby);
        environment(&mut command, &root, &row);
        let done = command.args(["-e", &format!("{LOADER}\nbegin; c = RuboCop::ConfigLoader.configuration_from_file(ARGV[0]); print JSON.generate(enabled: c.for_cop(ARGV[1])['Enabled']); rescue RuboCop::Error => e; print JSON.generate(error: e.message); end\n"), "--"])
            .arg(root.join(config_name)).arg(cop).output().unwrap();
        assert!(done.status.success(), "{}: {done:?}", row["id"]);
        assert!(done.stderr.is_empty(), "{}: {done:?}", row["id"]);
        let expected: Value = serde_json::from_slice(&done.stdout).unwrap();
        let actual = match Config::load(Some(&root.join(config_name)), &root) {
            Ok(config) => serde_json::json!({"enabled": config.rule_enabled(cop)}),
            Err(error) => serde_json::json!({"error": normalize_root(error.to_string(), &root)}),
        };
        assert_eq!(actual, expected, "{}", row["id"]);
        kept(&root, before, &row);
    }
}
