//! optionに見えるconfig値の消費を、実本家の通常検査・help dispatchで比較する。
//! helpは終了・dispatchを、値不足の対照は拒否終了を検査し、本文の全文一致へ数えない。
//! 真の値不足に対する診断文面と改行の差は#265で追跡する。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use serde_json::Value;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const CONFIG: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\nLayout/SpaceAroundOperators:\n  Enabled: true\n";
const VALUES: &[&str] = &[
    "--only",
    "--except",
    "--require",
    "--cache-root",
    "--help",
    "--version",
    "--",
    "-",
];

fn environment(command: &mut Command, root: &Path) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
        "SONICOP_PROFILE",
    ] {
        command.env_remove(key);
    }
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .env("RUBOCOP_CACHE_ROOT", root.join("cache"));
}

fn ruby(root: &Path) -> Option<PathBuf> {
    let path =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&path);
    environment(&mut command, root);
    let output = command.args(["-e", LOADER]).output();
    if output.as_ref().is_ok_and(|output| output.status.success()) {
        Some(path)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定Ruby/RuboCopが必要です: {output:?}"
        );
        eprintln!("未計測: 実Ruby/RuboCopがないためconfig値の比較を成功へ数えません");
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

#[derive(Clone, Copy)]
enum Mode {
    Inspect,
    Help,
    Missing,
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn cases() -> Vec<(String, Vec<String>, Mode)> {
    let mut cases = Vec::new();
    for value in VALUES {
        cases.push((
            format!("help-{value}"),
            words(&["--config", "config.yml", "--config", value, "--help"]),
            Mode::Help,
        ));
        cases.push((
            format!("inspect-{value}"),
            words(&[
                "--config", value, "--cache", "false", "--format", "json", "a.rb",
            ]),
            Mode::Inspect,
        ));
    }
    for (name, args, mode) in [
        (
            "equals-help",
            words(&["--config=--only", "--help"]),
            Mode::Help,
        ),
        ("short-help", words(&["-c", "--only", "--help"]), Mode::Help),
        ("attached-help", words(&["-c--only", "--help"]), Mode::Help),
        ("empty-help", words(&["--config", "", "--help"]), Mode::Help),
        ("missing", words(&["--config"]), Mode::Missing),
        (
            "root-value-help",
            words(&["--cache", "false", "--config", "--cache-root", "--help"]),
            Mode::Help,
        ),
        (
            "equals-inspect",
            words(&[
                "--config=--only",
                "--cache",
                "false",
                "--format",
                "json",
                "a.rb",
            ]),
            Mode::Inspect,
        ),
        (
            "short-inspect",
            words(&[
                "-c", "--only", "--cache", "false", "--format", "json", "a.rb",
            ]),
            Mode::Inspect,
        ),
        (
            "attached-inspect",
            words(&["-c--only", "--cache", "false", "--format", "json", "a.rb"]),
            Mode::Inspect,
        ),
        (
            "root-value-inspect",
            words(&[
                "--cache",
                "false",
                "--config",
                "--cache-root",
                "--format",
                "json",
                "a.rb",
            ]),
            Mode::Inspect,
        ),
        // このrequireは値。feature.rbは実行せず、通常の明示fileとして検査する。
        (
            "require-value-marker",
            words(&[
                "--config",
                "--require",
                "feature.rb",
                "--cache",
                "false",
                "--format",
                "json",
                "a.rb",
            ]),
            Mode::Inspect,
        ),
    ] {
        cases.push((name.to_owned(), args, mode));
    }
    assert_eq!(cases.len(), 27);
    cases
}

fn capture(root: &Path, ruby: &Path, reference: bool, args: &[String]) -> Output {
    let mut command = if reference {
        let mut command = Command::new(ruby);
        command.args([
            "-e",
            &format!("{LOADER}\nexit RuboCop::CLI.new.run(ARGV)\n"),
            "--",
        ]);
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.env("PATH", "");
        command
    };
    environment(&mut command, root);
    command.args(args).output().unwrap()
}

fn report(output: &Output, reference: bool) -> Value {
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        [Some(0), Some(1)].contains(&output.status.code()),
        "{output:?}"
    );
    assert_eq!(output.stdout.first(), Some(&b'{'), "{output:?}");
    assert_eq!(output.stdout.last(), Some(&b'}'), "{output:?}");
    let mut report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
    assert_eq!(
        report["metadata"]["ruby_engine"],
        if reference { "ruby" } else { "sonicop" }
    );
    for key in [
        "sonicop_version",
        "ruby_engine",
        "ruby_version",
        "ruby_platform",
    ] {
        report["metadata"].as_object_mut().unwrap().remove(key);
    }
    report
}

fn contains(bytes: &[u8], text: &[u8]) -> bool {
    bytes.windows(text.len()).any(|part| part == text)
}

#[test]
fn option_like_configuration_values_match_real_rubocop_without_running_callbacks() {
    let directory = tempfile::tempdir().unwrap();
    let Some(ruby) = ruby(directory.path()) else {
        return;
    };
    for (index, (name, args, mode)) in cases().into_iter().enumerate() {
        let root = directory.path().join(index.to_string());
        fs::create_dir(&root).unwrap();
        let mut files = BTreeMap::from([
            ("Gemfile", Vec::new()),
            ("config.yml", CONFIG.as_bytes().to_vec()),
            ("a.rb", b"x=1\n".to_vec()),
            ("report.json", b"KEEP\0\r\n".to_vec()),
            (
                "feature.rb",
                b"File.write('unexpected-marker', 'BAD')\n".to_vec(),
            ),
        ]);
        for value in VALUES {
            files.insert(value, CONFIG.as_bytes().to_vec());
        }
        for (path, bytes) in &files {
            fs::write(root.join(path), bytes).unwrap();
        }
        let original: Vec<_> = files
            .keys()
            .map(|path| (*path, snapshot(&root.join(path))))
            .collect();
        let mut outputs = Vec::new();
        for reference in [true, false] {
            outputs.push(capture(&root, &ruby, reference, &args));
            assert!(
                !root.join("unexpected-marker").exists(),
                "{name} {reference}"
            );
            for (path, before) in &original {
                assert_eq!(
                    snapshot(&root.join(path)),
                    *before,
                    "{name} {reference} {path}"
                );
            }
        }
        let reference = &outputs[0];
        let native = &outputs[1];
        assert_eq!(
            native.status.code(),
            reference.status.code(),
            "{name}: {native:?}"
        );
        match mode {
            Mode::Inspect => assert_eq!(report(native, false), report(reference, true), "{name}"),
            Mode::Help => {
                for output in &outputs {
                    assert_eq!(output.status.code(), Some(0), "{name}: {output:?}");
                    assert!(output.stderr.is_empty(), "{name}: {output:?}");
                    assert!(
                        contains(&output.stdout, b"Usage:")
                            && contains(&output.stdout, b"--config"),
                        "{name}: {output:?}"
                    );
                }
            }
            Mode::Missing => {
                for output in &outputs {
                    assert_eq!(output.status.code(), Some(2));
                    assert!(output.stdout.is_empty());
                    assert!(contains(&output.stderr, b"--config"));
                }
            }
        }
    }
}
