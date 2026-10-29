//! 実require表記と実class登録を直交させ、basenameの推測を成功へ混ぜない。

use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn actual_ruby() -> Option<PathBuf> {
    let child = Command::new("ruby")
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .env_remove("RUBOCOP_OPTS")
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .env_remove("NO_COLOR")
        .args([
            "-rrbconfig",
            "-e",
            "gem 'rubocop', '1.89.0'; require 'rubocop'; print RbConfig.ruby",
        ])
        .output()
        .ok()?;
    child
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8(child.stdout).unwrap()))
}

#[test]
fn real_requires_and_actual_registry_control_the_four_configuration_results() {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/runtime_requires.json")).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["native_invocations"], 0);
    let cases = value["cases"].as_array().unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "relative-empty",
            "exact-empty",
            "relative-registered",
            "exact-registered"
        ]
    );
    let Some(ruby) = actual_ruby() else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "実runtime必須gateに固定RuboCopがありません"
        );
        // ローカルでgemが無い場合も、未実行を4条件の互換成功として数えない。
        eprintln!(
            "未計測: Ruby + RuboCop 1.89.0 が無いためruntime requireの実4条件を実行できません"
        );
        return;
    };
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::write(&path, text.as_str().unwrap()).unwrap();
            let metadata = fs::metadata(&path).unwrap();
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                metadata.modified().unwrap(),
                metadata.permissions().readonly(),
            ));
        }
        let mut command = Command::new(&ruby);
        command
            .current_dir(directory.path())
            .arg("-I")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib"));
        command.args(
            case["ruby_args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        command.arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("exe/sonicop"));
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        command.env("SONICOP_BINARY", env!("CARGO_BIN_EXE_sonicop"));
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            case["cli"]["exit"].as_i64().map(|value| value as i32),
            "{}",
            case["id"]
        );
        let mut stderr = bytes(&case["cli"]["stderr_hex"]);
        if !cfg!(windows) {
            stderr = String::from_utf8(stderr)
                .unwrap()
                .replace("\r\n", "\n")
                .into_bytes();
        }
        assert_eq!(output.stderr, stderr, "{}", case["id"]);
        if case["cli"]["exit"] == 0 {
            let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            let mut expected: Value =
                serde_json::from_slice(&bytes(&case["cli"]["stdout_hex"])).unwrap();
            actual.as_object_mut().unwrap().remove("metadata");
            expected.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, expected);
        } else {
            assert!(output.stdout.is_empty());
        }
        for (path, original, modified, readonly) in originals {
            assert_eq!(fs::read(&path).unwrap(), original);
            let metadata = fs::metadata(path).unwrap();
            assert_eq!(metadata.modified().unwrap(), modified);
            assert_eq!(metadata.permissions().readonly(), readonly);
        }
        assert!(!directory.path().join("report.json").exists());
    }
}

#[test]
fn huge_wrapper_load_path_does_not_exhaust_exec_environment_or_leave_handoff_files() {
    let Some(ruby) = actual_ruby() else {
        assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
        eprintln!("未計測: wrapperの実Rubyがありません");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    for args in ["['--version']", "['--help']", "['--no-such-option']"] {
        let mut command = Command::new(&ruby);
        command.args(["-I"]).arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib"))
            .args(["-rsonicop", "-e"])
            .arg(format!("$LOAD_PATH.concat(Array.new(3000) {{ |i| '/sonicop-large-path/' + ('x' * 90) + i.to_s }}); Sonicop::Runner.run({args})"))
            .env("SONICOP_BINARY", env!("CARGO_BIN_EXE_sonicop"))
            .env("SONICOP_RUNTIME_RUBY", directory.path().join("missing-ruby"));
        for key in ["TMPDIR", "TMP", "TEMP"] {
            command.env(key, directory.path());
        }
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
        ] {
            command.env_remove(key);
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(if args.contains("no-such") { 2 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if args.contains("version") {
            assert_eq!(
                String::from_utf8(output.stdout).unwrap().trim(),
                env!("CARGO_PKG_VERSION")
            );
        }
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            0,
            "handoffが残っています"
        );
    }
}

#[test]
fn quiet_library_metadata_comes_from_the_real_four_api_observations() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/runtime_requires.json")).unwrap();
    if let Ok(id) = std::env::var("SONICOP_TEST_RUNTIME_API_CASE") {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap();
        let cwd = std::env::current_dir().unwrap();
        let config = sonicop::config::Config::load(Some(&cwd.join("config.yml")), &cwd).unwrap();
        assert_eq!(
            serde_json::to_value(config.loaded_features()).unwrap(),
            case["api"]["loaded_features"]
        );
        assert_eq!(
            serde_json::to_value(config.loaded_plugin_names()).unwrap(),
            case["api"]["loaded_plugins"]
        );
        let names = config
            .ruby_registered_cop_names()
            .iter()
            .filter(|name| name.starts_with("Performance/"))
            .collect::<Vec<_>>();
        assert_eq!(
            serde_json::to_value(names).unwrap(),
            case["api"]["performance_names"]
        );
        // quiet APIは実登録metadataを返し、従来どおりCLIの廃止fatalを新しく伝播しない。
        assert_eq!(
            config.cop_value::<bool>("Performance/CounterProbe", "Enabled"),
            Some(false)
        );
        return;
    }
    let Some(ruby) = actual_ruby() else {
        assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
        eprintln!("未計測: quiet runtime APIの実Rubyがありません");
        return;
    };
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::write(&path, text.as_str().unwrap()).unwrap();
            let metadata = fs::metadata(&path).unwrap();
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                metadata.modified().unwrap(),
                metadata.permissions().readonly(),
            ));
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "quiet_library_metadata_comes_from_the_real_four_api_observations",
                "--nocapture",
            ])
            .current_dir(directory.path())
            .env(
                "SONICOP_TEST_RUNTIME_API_CASE",
                case["id"].as_str().unwrap(),
            )
            .env("SONICOP_RUNTIME_RUBY", &ruby)
            .env("RUBYLIB", ".");
        for key in [
            "RUBYOPT",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
            "SONICOP_RUNTIME_HANDOFF",
            "SONICOP_RUNTIME_HANDOFF_TOKEN",
        ] {
            command.env_remove(key);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}: {} {}",
            case["id"],
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for (path, bytes, modified, readonly) in originals {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            let metadata = fs::metadata(&path).unwrap();
            assert_eq!(metadata.modified().unwrap(), modified);
            assert_eq!(metadata.permissions().readonly(), readonly);
        }
    }
}

#[test]
fn runtime_boundaries_follow_the_measured_order_defaults_plugins_and_io() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/runtime_requires_boundaries.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "once-inherit",
            "order-inherit",
            "cli-before-config",
            "cli-version",
            "config-stdin",
            "cli-stdin",
            "dynamic-bare",
            "late-bare",
            "nested-only",
            "nested-except",
            "root-first-only",
            "exit-seven",
            "abort-once",
            "interrupt",
            "at-exit-seven",
            "at-exit-error",
            "at-exit-after-error",
            "default-getter",
            "default-setter",
            "config-plugin",
            "cli-plugin",
            "cli-stdin-partial",
            "inherited-fatal-before-later-require",
            "inherited-relative-origin",
            "dynamic-ambiguous"
        ]
    );
    let Some(ruby) = actual_ruby() else {
        assert_ne!(std::env::var("SONICOP_RUNTIME_TESTS").as_deref(), Ok("1"));
        eprintln!("未計測: runtime境界25条件の実Rubyがありません");
        return;
    };
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut original = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text.as_str().unwrap()).unwrap();
            let metadata = fs::metadata(&path).unwrap();
            original.push((
                path.clone(),
                fs::read(&path).unwrap(),
                metadata.modified().unwrap(),
                metadata.permissions().readonly(),
            ));
        }
        let mut command = Command::new(&ruby);
        command
            .current_dir(directory.path())
            .arg("-I")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib"))
            .args(
                case["ruby_args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap()),
            )
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("exe/sonicop"))
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap()),
            )
            .env("SONICOP_BINARY", env!("CARGO_BIN_EXE_sonicop"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = case["stdin"].as_str() {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|value| value as i32),
            "{}: {}",
            case["id"],
            String::from_utf8_lossy(&output.stderr)
        );
        let root = if cfg!(target_os = "macos") {
            fs::canonicalize(directory.path()).unwrap()
        } else {
            directory.path().to_path_buf()
        };
        let root = root.to_string_lossy().replace('\\', "/");
        let native = |value: &[u8]| {
            let text = String::from_utf8(value.to_vec())
                .unwrap()
                .replace(&root, "<root>");
            if cfg!(windows) {
                text
            } else {
                text.replace("\r\n", "\n")
            }
        };
        let expected = |key| native(&bytes(&case[key]));
        let mut expected_stderr = expected("stderr_hex");
        if case["comparison"] == "native_cop_boundary" {
            expected_stderr.push_str("warning: 1 enabled cop not implemented by Sonicop; nothing was checked for it: Probe/Late\n");
        }
        assert_eq!(
            native(&output.stderr),
            expected_stderr,
            "{} stderr",
            case["id"]
        );
        let actual_stdout = native(&output.stdout);
        let expected_stdout = expected("stdout_hex");
        if let Some(index) = expected_stdout.find("{\"metadata\"") {
            assert_eq!(
                &actual_stdout[..index],
                &expected_stdout[..index],
                "{} prefix",
                case["id"]
            );
            let mut actual: Value = serde_json::from_str(&actual_stdout[index..]).unwrap();
            let mut expected: Value = serde_json::from_str(&expected_stdout[index..]).unwrap();
            assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
            assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
            actual.as_object_mut().unwrap().remove("metadata");
            expected.as_object_mut().unwrap().remove("metadata");
            assert_eq!(actual, expected, "{} JSON", case["id"]);
        } else if case["comparison"] == "version_identity" {
            assert_eq!(
                actual_stdout,
                expected_stdout.replace("1.89.0", env!("CARGO_PKG_VERSION")),
                "{}",
                case["id"]
            );
        } else {
            assert_eq!(actual_stdout, expected_stdout, "{} stdout", case["id"]);
        }
        for (path, bytes, modified, readonly) in original {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            let metadata = fs::metadata(&path).unwrap();
            assert_eq!(metadata.modified().unwrap(), modified);
            assert_eq!(metadata.permissions().readonly(), readonly);
        }
        assert!(!directory.path().join("report.json").exists());
    }
}

#[test]
fn normal_native_run_does_not_require_ruby_and_explicit_extensions_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let base = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";
    fs::write(directory.path().join("a.rb"), b"x=1\n").unwrap();
    fs::write(directory.path().join("config.yml"), base).unwrap();
    fs::write(directory.path().join("report.json"), b"KEEP").unwrap();
    let before = fs::metadata(directory.path().join("report.json"))
        .unwrap()
        .modified()
        .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_sonicop"))
            .current_dir(directory.path())
            .env("PATH", "")
            .env(
                "SONICOP_RUNTIME_RUBY",
                directory.path().join("missing-ruby"),
            )
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--format",
                "json",
                "a.rb",
            ])
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["metadata"]["ruby_engine"], "sonicop");
    assert_eq!(value["summary"]["offense_count"], 0);
    fs::write(
        directory.path().join("config.yml"),
        format!("{base}require: ./extension.rb\n"),
    )
    .unwrap();
    let output = run();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not start Ruby extension runtime")
    );
    assert_eq!(
        fs::read(directory.path().join("report.json")).unwrap(),
        b"KEEP"
    );
    assert_eq!(
        fs::metadata(directory.path().join("report.json"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
}
