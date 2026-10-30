//! NewCopsの許容型・検証段階・色と、実登録した部門を本家の記録へ照合する。

use std::{fs, path::PathBuf, process::Command, sync::OnceLock};

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

fn host(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        // Windows参照のreport輸送だけをhostの改行へ移し、原本bytesは変えない。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn fixture() -> Value {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/new_cops_validation.json")).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["coverage"]["subset"], false);
    assert_eq!(value["cases"].as_array().unwrap().len(), 78);
    value
}

fn report(raw: &[u8], engine: &str) -> Value {
    let mut value: Value = serde_json::from_slice(raw).unwrap();
    assert_eq!(value["metadata"]["ruby_engine"], engine);
    assert_eq!(value["metadata"]["rubocop_version"], "1.89.0");
    if engine == "ruby" {
        assert_eq!(value["metadata"]["ruby_version"], "4.0.7");
    }
    value.as_object_mut().unwrap().remove("metadata");
    value
}

fn actual_ruby() -> Option<&'static PathBuf> {
    static RUBY: OnceLock<Option<PathBuf>> = OnceLock::new();
    RUBY.get_or_init(|| {
        let executable = std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into());
        let result = Command::new(executable)
            .env_remove("RUBYOPT")
            .env_remove("RUBYLIB")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env_remove("NO_COLOR")
            // 参照記録はWindowsの4.0.7、実runtimeのgateはmise.tomlのOS別固定版を使う。
            .args(["-rrbconfig", "-e", "gem 'rubocop', '1.89.0'; require 'rubocop'; abort 'wrong Ruby' unless RUBY_VERSION == ARGV.fetch(0); print RbConfig.ruby", if cfg!(windows) { "4.0.7" } else { "4.0.8" }])
            .output().ok()?;
        result.status.success().then(|| PathBuf::from(String::from_utf8(result.stdout).unwrap()))
    }).as_ref()
}

fn inspect(case: &Value) -> bool {
    let runtime = case["requires_ruby"] == true;
    let ruby = if runtime {
        let Some(ruby) = actual_ruby() else {
            assert_ne!(
                std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
                Ok("1"),
                "実runtime必須gateに固定Ruby/RuboCopがありません"
            );
            eprintln!(
                "未計測: 実Rubyが無いため {} は互換成功へ数えません",
                case["id"]
            );
            return false;
        };
        Some(ruby)
    } else {
        None
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut originals = Vec::new();
    for (name, text) in case["files"].as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
        let info = fs::metadata(&path).unwrap();
        originals.push((
            path.clone(),
            fs::read(&path).unwrap(),
            info.modified().unwrap(),
            info.permissions(),
        ));
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("XDG_CACHE_HOME", root.join("cache"));
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
    ] {
        command.env_remove(key);
    }
    if let Some(ruby) = ruby {
        command.env("SONICOP_RUNTIME_RUBY", ruby);
    } else {
        command.env("PATH", "");
    }
    for (key, value) in case["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    command.args(
        case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap()),
    );
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code().map(i64::from),
        case["exit"].as_i64(),
        "{}: {output:?}",
        case["id"]
    );
    assert_eq!(
        output.stderr,
        host(bytes(&case["stderr_hex"])),
        "{} stderr",
        case["id"]
    );
    if case["kind"] == "accepted" || case["comparison"] == "full-report" {
        // 152で揃えたempty-finished JSONも、全fieldsを比較する。
        assert_eq!(
            report(&output.stdout, "sonicop"),
            report(&bytes(&case["stdout_hex"]), "ruby"),
            "{} report",
            case["id"]
        );
    } else {
        assert_eq!(
            output.stdout,
            host(bytes(&case["stdout_hex"])),
            "{} stdout",
            case["id"]
        );
    }
    assert_eq!(
        fs::symlink_metadata(root.join("report.json")).is_ok(),
        case["out_exists"].as_bool().unwrap(),
        "{} output state",
        case["id"]
    );
    if case["out_hex"].is_string() {
        assert_eq!(
            fs::read(root.join("report.json")).unwrap(),
            bytes(&case["out_hex"])
        );
    }
    for (path, raw, modified, permissions) in originals {
        assert_eq!(fs::read(&path).unwrap(), raw, "{} source", case["id"]);
        let info = fs::metadata(path).unwrap();
        assert_eq!(info.modified().unwrap(), modified, "{} mtime", case["id"]);
        assert_eq!(info.permissions(), permissions, "{} mode", case["id"]);
    }
    true
}

#[test]
fn invalid_new_cops_values_are_rejected_before_output_and_early_exit() {
    let value = fixture();
    let mut checked = 0;
    let mut skipped = 0;
    for row in value["cases"].as_array().unwrap() {
        if row["kind"] == "new-cops-error" && row["comparison"] == "full" {
            if inspect(row) {
                checked += 1;
            } else {
                skipped += 1;
            }
        }
    }
    assert_eq!(checked + skipped, 53);
    assert!(skipped <= 1);
}

#[test]
fn accepted_types_and_earlier_errors_keep_the_real_reports() {
    let value = fixture();
    let mut checked = 0;
    let mut skipped = 0;
    for row in value["cases"].as_array().unwrap() {
        if row["kind"] != "new-cops-error" {
            if inspect(row) {
                checked += 1;
            } else {
                skipped += 1;
            }
        }
    }
    assert_eq!(checked + skipped, 21);
    assert!(skipped <= 1);
}

#[test]
fn late_validation_keeps_full_empty_finished_reports_and_color() {
    let value = fixture();
    let rows = value["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["comparison"] == "full-report")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 4);
    for row in rows {
        assert!(inspect(row));
    }
}

#[test]
fn quiet_config_loading_rejects_invalid_new_cops_without_ansi() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.yml");
    fs::write(&path, "AllCops:\n  NewCops: '1.85'\n").unwrap();
    let error = sonicop::config::Config::load(Some(&path), directory.path()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.starts_with("invalid 1.85 for `NewCops` found in "),
        "{message}"
    );
    assert!(
        message.ends_with("\nValid choices are: pending, disable, enable"),
        "{message}"
    );
    assert!(!message.contains('\u{1b}'));
}
