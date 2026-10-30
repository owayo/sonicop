//! 設定解決後の拒否とparser拒否を、実本家の全report・原本保持とともに検査する。
#![cfg(windows)]

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/target_ruby_validation.json")).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn environment(command: &mut Command, root: &Path) {
    command
        .current_dir(root)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("empty-config"));
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "RUBOCOP_CACHE_ROOT",
        "SONICOP_RUNTIME_RUBY",
        "SONICOP_PROFILE",
    ] {
        command.env_remove(key);
    }
}

fn bind(text: &str, root: &Path) -> String {
    text.replace("<CWD_FWD>", &root.to_string_lossy().replace('\\', "/"))
        .replace("<CWD>", &root.to_string_lossy())
}

#[test]
fn fifty_one_actual_cli_conditions_keep_validation_phase_and_originals() {
    let data = fixture();
    let rows = data["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 51);
    for row in rows {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let originals: Vec<_> = row["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, body)| {
                let path = root.join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let body = body.as_str().unwrap().as_bytes().to_vec();
                fs::write(&path, &body).unwrap();
                let info = fs::metadata(&path).unwrap();
                (path, body, info.modified().unwrap(), info.permissions())
            })
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        environment(&mut command, root);
        for (key, value) in row["environment"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        command.args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        );
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&bytes(&row["stdin_hex"]))
            .unwrap();
        let actual = child.wait_with_output().unwrap();
        let id = row["id"].as_str().unwrap();
        assert_eq!(
            actual.status.code().map(i64::from),
            row["exit"].as_i64(),
            "{id}: {actual:?}"
        );
        let expected = bytes(&row["stdout_hex"]);
        match row["comparison"].as_str().unwrap() {
            "version_identity" => {
                assert_eq!(expected, b"1.89.0\r\n", "{id}");
                assert_eq!(
                    actual.stdout,
                    format!("{}\r\n", env!("CARGO_PKG_VERSION")).as_bytes(),
                    "{id}"
                );
            }
            "verbose_identity" => {
                assert!(expected.starts_with(b"1.89.0 "), "{id}");
                let output = String::from_utf8(actual.stdout.clone()).unwrap();
                assert!(
                    output.starts_with(&format!(
                        "sonicop {} (RuboCop {} CLI, ",
                        env!("CARGO_PKG_VERSION"),
                        sonicop::RUBOCOP_COMPAT_VERSION
                    )),
                    "{id}: {output}"
                );
                assert!(
                    output.ends_with(&format!(
                        "analyzing as Ruby 99.0) [{} {}]\r\n",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    )),
                    "{id}: {output}"
                );
            }
            _ if expected.starts_with(b"{") => {
                assert!(actual.stdout.ends_with(b"}"), "{id}");
                let mut report: Value = serde_json::from_slice(&actual.stdout).unwrap();
                let mut reference: Value = serde_json::from_slice(&expected).unwrap();
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
            }
            _ => assert_eq!(actual.stdout, expected, "{id}"),
        }
        // Ruby内部のframe全文は参照へ残し、Native全文の成功へ数えない。
        let stderr = if row["comparison"] == "parser_header" {
            &row["stderr_header_template"]
        } else {
            &row["stderr_template"]
        };
        assert_eq!(
            actual.stderr,
            bind(stderr.as_str().unwrap(), root).as_bytes(),
            "{id}"
        );
        assert_eq!(
            root.join("report.json").symlink_metadata().is_ok(),
            row["report_present"].as_bool().unwrap(),
            "{id}"
        );
        for (path, body, mtime, mode) in originals {
            assert_eq!(fs::read(&path).unwrap(), body, "{id}");
            let info = fs::metadata(path).unwrap();
            assert_eq!(info.modified().unwrap(), mtime, "{id}");
            assert_eq!(info.permissions(), mode, "{id}");
        }
    }
}

#[test]
fn nine_actual_getter_values_are_checked_in_quiet_child_processes() {
    for (index, row) in fixture()["api"].as_array().unwrap().iter().enumerate() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("Gemfile"), "").unwrap();
        let gem_info = fs::metadata(root.join("Gemfile")).unwrap();
        let text = format!(
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: {}\n",
            row["scalar_yaml"].as_str().unwrap()
        );
        let path = root.join(".rubocop.yml");
        fs::write(&path, &text).unwrap();
        let info = fs::metadata(&path).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        environment(&mut command, root);
        let actual = command
            .args(["--exact", "quiet_getter_child", "--nocapture"])
            .env("SONICOP_TEST_TARGET_VALIDATION_ROW", index.to_string())
            .output()
            .unwrap();
        assert!(actual.status.success(), "{}: {actual:?}", row["id"]);
        assert!(fs::read(root.join("Gemfile")).unwrap().is_empty());
        assert_eq!(
            fs::metadata(root.join("Gemfile"))
                .unwrap()
                .modified()
                .unwrap(),
            gem_info.modified().unwrap()
        );
        assert_eq!(
            fs::metadata(root.join("Gemfile")).unwrap().permissions(),
            gem_info.permissions()
        );
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            info.modified().unwrap()
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions(),
            info.permissions()
        );
    }
}

#[test]
fn quiet_getter_child() {
    let Ok(index) = std::env::var("SONICOP_TEST_TARGET_VALIDATION_ROW") else {
        return;
    };
    let data = fixture();
    let row = &data["api"][index.parse::<usize>().unwrap()];
    let cwd = std::env::current_dir().unwrap();
    let config = sonicop::config::Config::load(Some(Path::new(".rubocop.yml")), &cwd).unwrap();
    assert_eq!(
        config.target_ruby_version().to_string(),
        row["getter_inspect"].as_str().unwrap()
    );
}
